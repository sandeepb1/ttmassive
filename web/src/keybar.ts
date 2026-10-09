import { applyMods, encodeKey, type KeyName, type Mods } from './keys';

type ModName = keyof Mods;
type ModState = 'off' | 'once' | 'locked';

const DOUBLE_TAP_MS = 350;
const REPEAT_DELAY_MS = 400;
const REPEAT_EVERY_MS = 60;
const MOVE_SLOP_PX = 10;

interface Btn {
  label: string;
  aria?: string;
  key?: KeyName;
  mod?: ModName;
  text?: string; // literal text, still subject to armed modifiers
  repeat?: boolean;
  glyph?: boolean; // symbol label: render larger
}

const ROW1: Btn[] = [
  { label: 'Esc', key: 'Esc' },
  { label: 'Ctrl', mod: 'ctrl' },
  { label: 'Alt', mod: 'alt' },
  { label: 'Tab', key: 'Tab' },
  { label: '←', aria: 'Left', key: 'Left', repeat: true, glyph: true },
  { label: '↓', aria: 'Down', key: 'Down', repeat: true, glyph: true },
  { label: '↑', aria: 'Up', key: 'Up', repeat: true, glyph: true },
  { label: '→', aria: 'Right', key: 'Right', repeat: true, glyph: true },
];

const ROW2: Btn[] = [
  { label: '^C', text: '\x03' },
  { label: '⇧Tab', aria: 'Shift Tab', key: 'ShiftTab' },
  { label: '|', text: '|' },
  { label: '~', text: '~' },
  { label: '/', text: '/' },
  { label: '-', text: '-' },
  { label: 'PgUp', key: 'PgUp', repeat: true },
  { label: 'PgDn', key: 'PgDn', repeat: true },
  { label: 'Home', key: 'Home' },
  { label: 'End', key: 'End' },
];

/**
 * Touch key bar. Ctrl/Alt are sticky: one tap arms the next key, a quick
 * second tap locks until tapped again. Swipe up to reveal the second row.
 */
export class KeyBar {
  private state: Record<ModName, ModState> = { ctrl: 'off', alt: 'off' };
  private lastTap: Record<ModName, number> = { ctrl: 0, alt: 0 };
  private modEls: Partial<Record<ModName, HTMLButtonElement>> = {};

  constructor(
    root: HTMLElement,
    private send: (bytes: Uint8Array<ArrayBuffer>) => void,
    private appCursor: () => boolean,
    private refocus: () => void,
  ) {
    root.replaceChildren(this.row(ROW1), this.row(ROW2));
  }

  /** Current modifiers as a plain object. */
  mods(): Mods {
    return { ctrl: this.state.ctrl !== 'off', alt: this.state.alt !== 'off' };
  }

  /** Run soft-keyboard text through the armed modifiers, then release one-shots. */
  transform(data: string): string {
    const out = applyMods(data, this.mods());
    this.releaseOneShots();
    return out;
  }

  private row(btns: Btn[]): HTMLElement {
    const row = document.createElement('div');
    row.className = 'kb-row';
    for (const b of btns) row.append(this.button(b));
    return row;
  }

  private button(b: Btn): HTMLButtonElement {
    const el = document.createElement('button');
    el.type = 'button';
    el.className = b.glyph ? 'kb-key kb-glyph' : 'kb-key';
    el.textContent = b.label;
    el.setAttribute('aria-label', b.aria ?? b.label);
    el.tabIndex = -1;
    if (b.mod) {
      this.modEls[b.mod] = el;
      el.setAttribute('aria-pressed', 'false');
    }

    // Keys fire on release so a swipe that starts on a key (to reveal row 2)
    // doesn't press it. Repeating keys fire once the hold delay passes instead.
    let delay = 0;
    let timer = 0;
    let down: { x: number; y: number } | null = null;
    let held = false;
    const stop = () => {
      window.clearTimeout(delay);
      window.clearInterval(timer);
      down = null;
      held = false;
    };

    el.addEventListener('pointerdown', (e) => {
      e.preventDefault(); // keep focus in the terminal so the soft keyboard stays up
      down = { x: e.clientX, y: e.clientY };
      if (b.repeat) {
        delay = window.setTimeout(() => {
          held = true;
          this.fire(b);
          timer = window.setInterval(() => this.fire(b), REPEAT_EVERY_MS);
        }, REPEAT_DELAY_MS);
      }
    });
    el.addEventListener('pointermove', (e) => {
      if (down && Math.hypot(e.clientX - down.x, e.clientY - down.y) > MOVE_SLOP_PX) stop();
    });
    el.addEventListener('pointerup', () => {
      const tapped = down !== null && !held;
      stop();
      if (!tapped) return;
      if (b.mod) this.tapModifier(b.mod);
      else this.fire(b);
    });
    for (const ev of ['pointercancel', 'pointerleave'] as const) el.addEventListener(ev, stop);
    el.addEventListener('contextmenu', (e) => e.preventDefault());
    return el;
  }

  private fire(b: Btn) {
    const mods = this.mods();
    const raw = b.key ? encodeKey(b.key, mods, this.appCursor()) : applyMods(b.text ?? '', mods);
    this.send(new TextEncoder().encode(raw));
    this.releaseOneShots();
    this.refocus();
  }

  private tapModifier(m: ModName) {
    const now = performance.now();
    const cur = this.state[m];
    if (cur === 'off') this.state[m] = 'once';
    else if (cur === 'once' && now - this.lastTap[m] < DOUBLE_TAP_MS) this.state[m] = 'locked';
    else this.state[m] = 'off';
    this.lastTap[m] = now;
    this.render();
    this.refocus();
  }

  private releaseOneShots() {
    let changed = false;
    for (const m of ['ctrl', 'alt'] as const) {
      if (this.state[m] === 'once') {
        this.state[m] = 'off';
        changed = true;
      }
    }
    if (changed) this.render();
  }

  private render() {
    for (const m of ['ctrl', 'alt'] as const) {
      const el = this.modEls[m];
      if (!el) continue;
      el.dataset.state = this.state[m];
      el.setAttribute('aria-pressed', String(this.state[m] !== 'off'));
    }
  }
}
