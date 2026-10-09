// App-wide settings, in a sheet opened from the foot of the session list: theme, text size,
// notifications and signing out. Nothing here belongs to one session.

import { el } from './dom';
import { warn } from './haptics';
import { disablePush, enablePush, pushState, sendTest, type PushState } from './push';
import { getUiScale, SCALE_MAX, SCALE_MIN, SCALE_STEP, setUiScale } from './scale';
import { Sheet } from './sheet';
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
}

/** A label with a minus button, a value and a plus button. */
function stepper(label: string, value: () => string, step: (dir: -1 | 1) => void, canStep: (dir: -1 | 1) => boolean): HTMLElement {
  const box = el('div', 'more-block');
  box.append(el('div', 'sheet-label', label));
  const wrap = el('div', 'stepper');
  const out = el('div', 'step-value');
  const minus = el('button', 'step-btn', '−');
  const plus = el('button', 'step-btn', '+');
  minus.type = plus.type = 'button';
  minus.setAttribute('aria-label', `Smaller ${label.toLowerCase()}`);
  plus.setAttribute('aria-label', `Larger ${label.toLowerCase()}`);
  const sync = () => {
    out.textContent = value();
    minus.disabled = !canStep(-1);
    plus.disabled = !canStep(1);
  };
  for (const [b, dir] of [[minus, -1], [plus, 1]] as const) {
    b.addEventListener('click', () => {
      step(dir);
      sync();
    });
  }
  sync();
  wrap.append(minus, out, plus);
  box.append(wrap);
  return box;
}

export class SettingsSheet extends Sheet {
  private note = el('div', 'sheet-hint');
  private notif = el('div', 'more-block');
  private tunnel = el('div', 'more-block');

  constructor(
    host: HTMLElement,
    private hooks: SettingsHooks,
  ) {
    super(host, '', 'Settings', hooks.onUnauthorized);
  }

  open() {
    const appearance = el('div', 'more-block');
    appearance.append(new ThemePicker().root);

    let scale = getUiScale();
    const text = stepper(
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
        stepper(
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

    const account = el('div', 'more-block');
    account.append(el('div', 'sheet-label', 'Account'));
    const out = el('button', 'sheet-row sheet-danger');
    out.type = 'button';
    out.append(el('span', 'sr-label', 'Sign out'));
    out.addEventListener('click', () => {
      warn();
      this.close();
      this.hooks.logout();
    });
    account.append(out);

    this.body.replaceChildren(...blocks, this.tunnel, this.notif, this.note, account);
    void this.renderNotif();
    void this.renderTunnel();
    this.show();
  }

  private say(text: string) {
    this.note.textContent = text;
  }

  /** The public address, when the server was started with `--tunnel`. */
  private async renderTunnel() {
    const t = await fetchTunnel();
    if (!t) return this.tunnel.replaceChildren();
    const row = el('div', 'drawer-chips');
    row.append(el('span', 'notif-state', tunnelStatus(t)));
    if (t.url) {
      const url = t.url;
      const open = el('a', 'chip', url.replace('https://', ''));
      open.href = url;
      open.target = '_blank';
      open.rel = 'noopener';
      const copy = el('button', 'chip', 'Copy');
      copy.type = 'button';
      copy.addEventListener('click', async () => {
        try {
          await navigator.clipboard.writeText(url);
          this.say('Address copied');
        } catch {
          this.say(url);
        }
      });
      row.append(open, copy);
    }
    this.tunnel.replaceChildren(el('div', 'sheet-label', 'Public address'), row);
  }

  private async renderNotif() {
    const state: PushState = await pushState().catch(() => 'unsupported');
    const label = el('div', 'sheet-label', 'Notifications');
    const row = el('div', 'drawer-chips');
    const text: Record<PushState, string> = {
      unsupported: 'Not supported here (needs HTTPS or localhost)',
      denied: 'Blocked in browser settings',
      off: 'Off',
      on: 'On: you will be alerted when an agent needs you',
    };
    row.append(el('span', 'notif-state', text[state]));
    if (state === 'off' || state === 'on') {
      const toggle = el('button', 'chip', state === 'on' ? 'Turn off' : 'Turn on');
      toggle.type = 'button';
      toggle.addEventListener('click', async () => {
        try {
          if (state === 'on') await disablePush();
          else await enablePush();
          this.say('');
        } catch (e) {
          this.say(e instanceof Error ? e.message : 'Failed');
        }
        await this.renderNotif();
      });
      row.append(toggle);
    }
    if (state === 'on') {
      const test = el('button', 'chip', 'Send test');
      test.type = 'button';
      test.addEventListener('click', async () => {
        try {
          const n = await sendTest(this.hooks.currentSession());
          this.say(n === 0 ? 'No devices subscribed' : `Test sent to ${n} device${n === 1 ? '' : 's'}`);
        } catch (e) {
          this.say(e instanceof Error ? e.message : 'Failed');
        }
      });
      row.append(test);
    }
    this.notif.replaceChildren(label, row);
  }
}
