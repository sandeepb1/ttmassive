// App-wide settings, in a sheet opened from the foot of the session list. A row of section chips (Appearance,
// Terminal, Sessions, Notifications, Account) picks which group of settings shows. Nothing here belongs to one session.

import { el } from './dom';
import { warn } from './haptics';
import {
  CONTRAST_LEVELS,
  CURSOR_STYLES,
  LETTER_SPACING,
  LINE_HEIGHT,
  getBellSound,
  getBoldBright,
  getCursorBlink,
  getCursorStyle,
  getLetterSpacing,
  getLigatures,
  getLineHeight,
  getMinContrast,
  getOptionMeta,
  getTermCustomFont,
  getTermFont,
  setBellSound,
  setBoldBright,
  setCursorBlink,
  setCursorStyle,
  setLetterSpacing,
  setLigatures,
  setLineHeight,
  setMinContrast,
  setOptionMeta,
  setTermCustomFont,
  setTermFont,
  type CursorStyle,
} from './prefs';
import { TERM_FONTS, cleanFamily, hasLigatures, prepareFont } from './term-fonts';
import { disablePush, enablePush, pushState, sendTest, type PushState } from './push';
import { getUiScale, SCALE_MAX, SCALE_MIN, SCALE_STEP, setUiScale } from './scale';
import { Sheet, row } from './sheet';
import { button } from './ui/button';
import { chip, chipClass, setChipOn } from './ui/chip';
import { input, select } from './ui/input';
import { sheetBlock, sheetHint, sheetLabel } from './ui/sheet-parts';
import { stepper } from './ui/stepper';
import { updatesBlock } from './server-update';
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
  const block = sheetBlock(sheetLabel(label), steps.el);
  block.classList.add('step-row'); // landscape.css puts the label and the control on one line
  return block;
}

/** A setting's name on the left and its control on the right. */
const settingRow = (label: string, control: HTMLElement): HTMLElement => {
  const r = el('div', 'flex items-center justify-between gap-3');
  r.append(el('span', 'text-md', label), control);
  return r;
};

/** A setting that is on or off: its name on the left and an On/Off chip on the right. */
function toggleRow(label: string, get: () => boolean, set: (on: boolean) => void): HTMLElement {
  const b = chip({ on: get() }, get() ? 'On' : 'Off');
  b.addEventListener('click', () => {
    set(!get());
    b.textContent = get() ? 'On' : 'Off';
    setChipOn(b, get());
  });
  return settingRow(label, b);
}

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

interface Section {
  id: string;
  label: string;
  blocks: HTMLElement[];
}

/** The section that was open last time, so the sheet comes back to where you were working. */
let lastSection = 'appearance';

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
    this.tall(); // one height for every section, so the sheet does not jump when you switch
    let scale = getUiScale();
    const interfaceText = stepControl(
      'Interface text',
      () => `${scale}%`,
      (dir) => {
        scale = Math.min(SCALE_MAX, Math.max(SCALE_MIN, scale + dir * SCALE_STEP));
        setUiScale(scale);
      },
      (dir) => (dir < 0 ? scale > SCALE_MIN : scale < SCALE_MAX),
    );

    const out = row('Sign out', { danger: true });
    out.addEventListener('click', () => {
      warn();
      this.close();
      this.hooks.logout();
    });

    const sections: Section[] = [
      { id: 'appearance', label: 'Appearance', blocks: [new ThemePicker().root, interfaceText] },
      { id: 'terminal', label: 'Terminal', blocks: [...this.terminalSizeBlock(), ...this.terminalLookBlocks(), this.keyboardBlock()] },
      { id: 'sessions', label: 'Sessions', blocks: [this.panesBlock()] },
      { id: 'notifications', label: 'Notifications', blocks: [this.notif, this.soundsBlock()] },
      { id: 'updates', label: 'Updates', blocks: [updatesBlock(this.host, () => this.close())] },
      { id: 'account', label: 'Account', blocks: [this.tunnel, sheetBlock(sheetLabel('Account'), out)] },
    ];
    this.renderSections(sections);
    void this.renderNotif();
    void this.renderTunnel();
    this.show();
  }

  /** A row of section chips that stays put, and under it the chosen section, which scrolls on its own. */
  private renderSections(sections: Section[]) {
    this.body.classList.add('settings-body'); // landscape.css turns the chips into a column beside the settings
    const nav = el('div', 'settings-nav flex shrink-0 gap-2 overflow-x-auto pb-3 pt-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden');
    nav.setAttribute('role', 'tablist');
    nav.setAttribute('aria-label', 'Settings sections');
    const panels = new Map<string, HTMLElement>();
    const tabs = new Map<string, HTMLButtonElement>();
    const scroller = el('div', 'flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto pb-2');
    const pick = (id: string, focus = false) => {
      lastSection = id;
      for (const [sid, tab] of tabs) {
        const on = sid === id;
        setChipOn(tab, on);
        tab.setAttribute('aria-selected', String(on));
        tab.tabIndex = on ? 0 : -1;
        panels.get(sid)!.hidden = !on;
      }
      scroller.scrollTop = 0;
      if (focus) tabs.get(id)?.focus();
      tabs.get(id)?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    };
    for (const sec of sections) {
      const tab = chip({ on: false }, sec.label);
      tab.removeAttribute('aria-pressed'); // a tab is selected, not pressed
      tab.setAttribute('role', 'tab');
      tab.id = `settings-tab-${sec.id}`;
      tab.setAttribute('aria-controls', `settings-panel-${sec.id}`);
      tab.addEventListener('click', () => pick(sec.id));
      tabs.set(sec.id, tab);
      nav.append(tab);
      const panel = el('div', 'flex flex-col gap-5');
      panel.id = `settings-panel-${sec.id}`;
      panel.setAttribute('role', 'tabpanel');
      panel.setAttribute('aria-labelledby', tab.id);
      panel.append(...sec.blocks);
      panels.set(sec.id, panel);
      scroller.append(panel);
    }
    nav.addEventListener('keydown', (e) => {
      const at = sections.findIndex((sec) => sec.id === lastSection);
      const to = e.key === 'ArrowRight' ? at + 1 : e.key === 'ArrowLeft' ? at - 1 : e.key === 'Home' ? 0 : e.key === 'End' ? sections.length - 1 : -1;
      if (to < 0) return;
      e.preventDefault();
      pick(sections[(to + sections.length) % sections.length]!.id, true);
    });
    scroller.append(this.note);
    this.body.replaceChildren(nav, scroller);
    pick(sections.some((sec) => sec.id === lastSection) ? lastSection : sections[0]!.id);
  }

  /** The terminal's text size, when a terminal is open (it is a property of that terminal). */
  private terminalSizeBlock(): HTMLElement[] {
    const term = this.hooks.terminalFont();
    if (!term) return [];
    let size = term.size;
    return [
      stepControl(
        'Text size',
        () => `${size} px`,
        (dir) => {
          size = Math.min(term.max, Math.max(term.min, size + dir));
          this.hooks.setTerminalFont(size);
        },
        (dir) => (dir < 0 ? size > term.min : size < term.max),
      ),
    ];
  }

  /** How the terminal looks, in groups: font, spacing, cursor, colours. Kept per device, applied to every open pane at once. */
  private terminalLookBlocks(): HTMLElement[] {
    const box = el('div', 'flex flex-col gap-5');
    const preview = el('div', 'overflow-x-auto whitespace-pre rounded-lg bg-secondary px-3 py-2.5 text-sm text-foreground');
    preview.textContent = 'const ok = a !== b && x >= 0 ? y => y : null;\n// -> => <= == === != :: 0O 1lI';
    preview.setAttribute('aria-label', 'Preview of the terminal font');
    let seq = 0;
    // The preview draws with the same font and ligature choice as the terminal (the browser's own ligatures here).
    const paint = async () => {
      const mine = ++seq;
      const stack = await prepareFont(getTermFont(), getTermCustomFont());
      if (mine !== seq) return;
      preview.style.fontFamily = stack;
      preview.style.fontVariantLigatures = getLigatures() && hasLigatures(getTermFont(), getTermCustomFont()) ? 'normal' : 'none';
      preview.style.lineHeight = String(getLineHeight());
      preview.style.letterSpacing = `${getLetterSpacing()}px`;
    };
    const render = () => {
      const id = getTermFont();
      const fontSel = select(TERM_FONTS.map((f) => ({ value: f.id, label: f.label })), { value: id, label: 'Terminal font', cls: 'w-auto' });
      fontSel.addEventListener('change', () => {
        setTermFont(fontSel.value);
        render();
      });
      const font: HTMLElement[] = [sheetLabel('Font'), settingRow('Typeface', fontSel)];
      if (id === 'custom') {
        const name = input({ value: getTermCustomFont(), placeholder: 'Font name, e.g. MesloLGS NF', label: 'Installed font name' });
        name.addEventListener('change', () => {
          name.value = cleanFamily(name.value);
          setTermCustomFont(name.value);
          render();
        });
        font.push(name, sheetHint('The name of a font installed on this device, such as a Nerd Font for prompt icons. Until it is found, JetBrains Mono is used.'));
      }
      const canLigate = hasLigatures(id, getTermCustomFont());
      font.push(
        preview,
        canLigate ? toggleRow('Ligatures', getLigatures, (on) => (setLigatures(on), void paint())) : sheetHint('This font has no ligatures.'),
      );
      const spacing = sheetBlock(
        stepControl(
          'Line height',
          () => getLineHeight().toFixed(2),
          (dir) => (setLineHeight(getLineHeight() + dir * LINE_HEIGHT.step), void paint()),
          (dir) => (dir < 0 ? getLineHeight() > LINE_HEIGHT.min + 0.001 : getLineHeight() < LINE_HEIGHT.max - 0.001),
        ),
        stepControl(
          'Letter spacing',
          () => `${getLetterSpacing()} px`,
          (dir) => (setLetterSpacing(getLetterSpacing() + dir * LETTER_SPACING.step), void paint()),
          (dir) => (dir < 0 ? getLetterSpacing() > LETTER_SPACING.min : getLetterSpacing() < LETTER_SPACING.max),
        ),
      );
      const cursor = select(CURSOR_STYLES.map((c) => ({ value: c, label: c[0]!.toUpperCase() + c.slice(1) })), { value: getCursorStyle(), label: 'Cursor', cls: 'w-auto' });
      cursor.addEventListener('change', () => setCursorStyle(cursor.value as CursorStyle));
      const contrast = select(CONTRAST_LEVELS.map((c) => ({ value: String(c.value), label: c.label })), { value: String(getMinContrast()), label: 'Text contrast', cls: 'w-auto' });
      contrast.addEventListener('change', () => setMinContrast(Number(contrast.value)));
      box.replaceChildren(
        sheetBlock(...font),
        spacing,
        sheetBlock(sheetLabel('Cursor'), settingRow('Style', cursor), toggleRow('Blinks', getCursorBlink, setCursorBlink)),
        sheetBlock(
          sheetLabel('Colours'),
          settingRow('Text contrast', contrast),
          sheetHint('Lifts text that is too faint against the background, like the grey hints in Claude and Codex.'),
          toggleRow('Bold text in bright colours', getBoldBright, setBoldBright),
        ),
      );
      void paint();
    };
    render();
    return [box];
  }

  /** Hardware-keyboard behaviour; kept per device because layouts and keyboards differ. */
  private keyboardBlock(): HTMLElement {
    return sheetBlock(
      sheetLabel('Keyboard'),
      toggleRow('Option key as Meta', getOptionMeta, setOptionMeta),
      sheetHint('Option+B/F jump by word and Option+Enter adds a new line. Turn off to type characters like é with Option.'),
    );
  }

  /** The bell from a session you are not looking at. */
  private soundsBlock(): HTMLElement {
    return sheetBlock(sheetLabel('Sounds'), toggleRow('Bell sound', getBellSound, setBellSound), sheetHint('A soft sound when a session you are not looking at rings the terminal bell.'));
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
        sheetLabel('Several sessions'),
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
