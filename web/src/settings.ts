// App-wide settings, in a sheet opened from the foot of the session list: theme, text size,
// notifications and signing out. Nothing here belongs to one session.

import { el } from './dom';
import { warn } from './haptics';
import { getBellSound, getOptionMeta, setBellSound, setOptionMeta } from './prefs';
import { disablePush, enablePush, pushState, sendTest, type PushState } from './push';
import { getUiScale, SCALE_MAX, SCALE_MIN, SCALE_STEP, setUiScale } from './scale';
import { Sheet, row } from './sheet';
import { button } from './ui/button';
import { chip, chipClass, setChipOn } from './ui/chip';
import { select } from './ui/input';
import { sheetBlock, sheetHint, sheetLabel } from './ui/sheet-parts';
import { stepper } from './ui/stepper';
import { PRESETS, type PresetId } from './tiling';
import { MAX_PANES_LIMIT, type LayoutSettings } from './workspace';
import { ThemePicker } from './theme-picker';
import { fetchTunnel, tunnelStatus } from './tunnel';

export interface SettingsHooks {
  onUnauthorized: () => void;
  logout: () => void;
  /** The open session, for the test notification. */
  currentSession: () => string;
  /** The terminal's font size now, or null when no terminal is open. */
  terminalFont: () => { size: number; min: number; max: number } | null;
  setTerminalFont: (px: number) => void;
  /** Pane preferences, kept on the server. */
  layoutSettings: () => LayoutSettings;
  setLayoutSettings: (s: LayoutSettings) => void;
}

/** A label with a minus button, a value and a plus button. */
function stepControl(label: string, value: () => string, step: (dir: -1 | 1) => void, canStep: (dir: -1 | 1) => boolean): HTMLElement {
  const steps = stepper({ less: `Smaller ${label.toLowerCase()}`, more: `Larger ${label.toLowerCase()}` }, (dir) => {
    step(dir);
    sync();
  });
  const sync = () => {
    steps.out.textContent = value();
    steps.minus.disabled = !canStep(-1);
    steps.plus.disabled = !canStep(1);
  };
  sync();
  return sheetBlock(sheetLabel(label), steps.el);
}

/** What this device reports about its screen, for working out layout problems. */
function displayReadout(): string {
  const probe = (css: string) => {
    const e = el('div');
    e.style.cssText = `position:fixed;visibility:hidden;pointer-events:none;${css}`;
    document.body.append(e);
    const r = e.getBoundingClientRect();
    e.remove();
    return r;
  };
  const inset = (side: string) => Math.round(probe(`top:0;left:0;height:env(safe-area-inset-${side});width:1px`).height);
  const vh = (unit: string) => Math.round(probe(`top:0;left:0;width:1px;height:100${unit}`).height);
  const vv = window.visualViewport;
  const app = document.getElementById('app')?.getBoundingClientRect();
  const standalone = matchMedia('(display-mode: standalone)').matches;
  return [
    `window ${innerWidth}×${innerHeight}`,
    `visual ${vv ? `${Math.round(vv.width)}×${Math.round(vv.height)} (top ${Math.round(vv.offsetTop)})` : 'n/a'}`,
    `screen ${screen.width}×${screen.height}`,
    `100vh ${vh('vh')}  dvh ${vh('dvh')}  svh ${vh('svh')}  lvh ${vh('lvh')}`,
    `inset top ${inset('top')}  bottom ${inset('bottom')}`,
    `#app ${app ? `${Math.round(app.width)}×${Math.round(app.height)} at ${Math.round(app.top)}` : 'n/a'}`,
    `installed app ${standalone}`,
  ].join('\n');
}

/** A setting's name on the left and its control on the right. */
const settingRow = (label: string, control: HTMLElement): HTMLElement => {
  const r = el('div', 'flex items-center justify-between gap-3');
  r.append(el('span', 'text-md', label), control);
  return r;
};

/** A labelled pick-list of grids for a second pane to start from. */
function gridSelect(label: string, value: PresetId, pick: (id: PresetId) => void, disabled: boolean): HTMLElement {
  const sel = select(
    PRESETS.filter((p) => p.id !== 'single').map((p) => ({ value: p.id, label: p.label })),
    { value, label, cls: 'w-auto' },
  );
  sel.disabled = disabled;
  sel.addEventListener('change', () => pick(sel.value as PresetId));
  return settingRow(label, sel);
}

export class SettingsSheet extends Sheet {
  private note = sheetHint('');
  private notif = sheetBlock();
  private tunnel = sheetBlock();

  constructor(
    host: HTMLElement,
    private hooks: SettingsHooks,
  ) {
    super(host, '', 'Settings', hooks.onUnauthorized);
  }

  open() {
    const appearance = sheetBlock(new ThemePicker().root);

    let scale = getUiScale();
    const text = stepControl(
      'Interface text',
      () => `${scale}%`,
      (dir) => {
        scale = Math.min(SCALE_MAX, Math.max(SCALE_MIN, scale + dir * SCALE_STEP));
        setUiScale(scale);
      },
      (dir) => (dir < 0 ? scale > SCALE_MIN : scale < SCALE_MAX),
    );

    const blocks: HTMLElement[] = [appearance, text];
    const term = this.hooks.terminalFont();
    if (term) {
      let size = term.size;
      blocks.push(
        stepControl(
          'Terminal text',
          () => `${size} px`,
          (dir) => {
            size = Math.min(term.max, Math.max(term.min, size + dir));
            this.hooks.setTerminalFont(size);
          },
          (dir) => (dir < 0 ? size > term.min : size < term.max),
        ),
      );
    }

    blocks.push(this.keyboardBlock(), this.panesBlock());

    const out = row('Sign out', { danger: true });
    out.addEventListener('click', () => {
      warn();
      this.close();
      this.hooks.logout();
    });
    const account = sheetBlock(sheetLabel('Account'), out);

    const display = sheetBlock(sheetLabel('Display'), el('pre', 'm-0 select-text whitespace-pre-wrap font-mono text-xs text-muted-foreground', displayReadout()));
    this.body.replaceChildren(...blocks, this.tunnel, this.notif, this.note, account, display);
    void this.renderNotif();
    void this.renderTunnel();
    this.show();
  }

  /** Hardware-keyboard and bell behaviour; kept per device because layouts and keyboards differ. */
  private keyboardBlock(): HTMLElement {
    const toggle = (label: string, get: () => boolean, set: (on: boolean) => void) => {
      const b = chip({ on: get() }, get() ? 'On' : 'Off');
      b.addEventListener('click', () => {
        set(!get());
        b.textContent = get() ? 'On' : 'Off';
        setChipOn(b, get());
      });
      return settingRow(label, b);
    };
    return sheetBlock(
      sheetLabel('Keyboard'),
      toggle('Option key as Meta', getOptionMeta, setOptionMeta),
      sheetHint('Option+B/F jump by word and Option+Enter adds a new line. Turn off to type characters like é with Option.'),
      toggle('Bell sound', getBellSound, setBellSound),
    );
  }

  /** Several panes at once: on or off, how many, and which grid to start from. */
  private panesBlock(): HTMLElement {
    const box = sheetBlock();
    const cur: LayoutSettings = { ...this.hooks.layoutSettings() };
    const commit = () => this.hooks.setLayoutSettings({ ...cur });
    const render = () => {
      const toggle = chip({ on: cur.multi }, cur.multi ? 'On' : 'Off');
      toggle.addEventListener('click', () => {
        cur.multi = !cur.multi;
        commit();
        render();
      });
      const head = settingRow('Show several sessions at once', toggle);
      const count = stepControl(
        'Most panes',
        () => String(cur.maxPanes),
        (dir) => {
          cur.maxPanes = Math.min(MAX_PANES_LIMIT, Math.max(1, cur.maxPanes + dir));
          commit();
        },
        (dir) => cur.multi && (dir < 0 ? cur.maxPanes > 1 : cur.maxPanes < MAX_PANES_LIMIT),
      );
      box.replaceChildren(
        sheetLabel('Panes'),
        head,
        count,
        gridSelect('Starting grid, big screen', cur.desktop, (id) => ((cur.desktop = id), commit()), !cur.multi),
        gridSelect('Starting grid, tablet', cur.tablet, (id) => ((cur.tablet = id), commit()), !cur.multi),
        sheetHint('Tiles that do not fit your screen become tabs. Ctrl+Alt+arrows move between tiles.'),
      );
    };
    render();
    return box;
  }

  private say(text: string) {
    this.note.textContent = text;
  }

  /** The public address, when the server was started with `--tunnel`. */
  private async renderTunnel() {
    const t = await fetchTunnel();
    if (!t) return this.tunnel.replaceChildren();
    const row = el('div', 'flex flex-wrap items-center gap-2');
    row.append(el('span', 'text-sm text-muted-foreground', tunnelStatus(t)));
    if (t.url) {
      const url = t.url;
      const open = el('a', chipClass(), url.replace('https://', ''));
      open.href = url;
      open.target = '_blank';
      open.rel = 'noopener';
      const copy = button({ variant: 'secondary', size: 'sm', onClick: async () => {
        try {
          await navigator.clipboard.writeText(url);
          this.say('Address copied');
        } catch {
          this.say(url);
        }
      } }, 'Copy');
      row.append(open, copy);
    }
    this.tunnel.replaceChildren(sheetLabel('Public address'), row);
  }

  private async renderNotif() {
    const state: PushState = await pushState().catch(() => 'unsupported');
    const label = sheetLabel('Notifications');
    const row = el('div', 'flex flex-wrap items-center gap-2');
    const text: Record<PushState, string> = {
      unsupported: 'Not supported here (needs HTTPS or localhost)',
      denied: 'Blocked in browser settings',
      off: 'Off',
      on: 'On: you will be alerted when an agent needs you',
    };
    row.append(el('span', 'text-sm text-muted-foreground', text[state]));
    if (state === 'off' || state === 'on') {
      row.append(button({ variant: 'secondary', size: 'sm', onClick: async () => {
        try {
          if (state === 'on') await disablePush();
          else await enablePush();
          this.say('');
        } catch (e) {
          this.say(e instanceof Error ? e.message : 'Failed');
        }
        await this.renderNotif();
      } }, state === 'on' ? 'Turn off' : 'Turn on'));
    }
    if (state === 'on') {
      row.append(button({ variant: 'secondary', size: 'sm', onClick: async () => {
        try {
          const n = await sendTest(this.hooks.currentSession());
          this.say(n === 0 ? 'No devices subscribed' : `Test sent to ${n} device${n === 1 ? '' : 's'}`);
        } catch (e) {
          this.say(e instanceof Error ? e.message : 'Failed');
        }
      } }, 'Send test'));
    }
    this.notif.replaceChildren(label, row);
  }
}
