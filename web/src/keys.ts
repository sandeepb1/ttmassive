// Pure key → byte encoding for the touch keybar. No DOM access, so it is unit-testable.

export interface Mods {
  ctrl: boolean;
  alt: boolean;
}

export type KeyName =
  | 'Esc' | 'Tab' | 'ShiftTab' | 'Up' | 'Down' | 'Left' | 'Right'
  | 'PgUp' | 'PgDn' | 'Home' | 'End' | 'Enter';

const ESC = '\x1b';
const CSI_FINAL: Record<string, string> = { Up: 'A', Down: 'B', Right: 'C', Left: 'D', Home: 'H', End: 'F' };
const CSI_TILDE: Record<string, number> = { PgUp: 5, PgDn: 6 };

/** xterm modifier parameter: 1 + (shift=1, alt=2, ctrl=4). */
function modParam(m: Mods): number {
  return 1 + (m.alt ? 2 : 0) + (m.ctrl ? 4 : 0);
}

/**
 * Encode a named key. `appCursor` is DECCKM (application cursor keys), which
 * full-screen TUIs commonly enable; arrows then use SS3 instead of CSI.
 */
export function encodeKey(key: KeyName, m: Mods, appCursor = false): string {
  const modded = m.ctrl || m.alt;
  switch (key) {
    case 'Esc': return ESC;
    case 'Enter': return m.alt ? ESC + '\r' : '\r';
    case 'Tab': return m.alt ? ESC + '\t' : '\t';
    case 'ShiftTab': return `${ESC}[Z`;
    case 'PgUp':
    case 'PgDn': {
      const n = CSI_TILDE[key];
      return modded ? `${ESC}[${n};${modParam(m)}~` : `${ESC}[${n}~`;
    }
    default: {
      const f = CSI_FINAL[key];
      if (modded) return `${ESC}[1;${modParam(m)}${f}`;
      return appCursor ? `${ESC}O${f}` : `${ESC}[${f}`;
    }
  }
}

/** Apply Ctrl to a single typed character; returns null when there is no control form. */
function ctrlChar(ch: string): string | null {
  if (ch.length !== 1) return null;
  const c = ch.charCodeAt(0);
  if (c >= 0x61 && c <= 0x7a) return String.fromCharCode(c - 0x60); // a-z
  if (c >= 0x40 && c <= 0x5f) return String.fromCharCode(c - 0x40); // @ A-Z [ \ ] ^ _
  if (ch === ' ') return '\x00';
  if (ch === '?') return '\x7f';
  return null;
}

/**
 * Apply armed modifiers to text coming from the soft keyboard. Only single
 * characters are transformed; pasted/IME text passes through untouched.
 */
export function applyMods(data: string, m: Mods): string {
  if ((!m.ctrl && !m.alt) || [...data].length !== 1) return data;
  let out = data;
  if (m.ctrl) out = ctrlChar(data) ?? data;
  return m.alt ? ESC + out : out;
}

/**
 * What Shift+Enter (or Alt+Enter) sends to start a new line instead of submitting. Claude Code reads
 * ESC CR (what its /terminal-setup installs); Codex reads Ctrl+J. A shell keeps a plain Enter.
 */
export function newlineFor(agent: 'claude' | 'codex' | null): string | null {
  if (agent === 'claude') return '\x1b\r';
  if (agent === 'codex') return '\n';
  return null;
}
