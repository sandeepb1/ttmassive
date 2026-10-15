// Per-device terminal preferences. Keyboards and layouts differ between a Mac, an iPad and a phone,
// so these live in the browser rather than on the server.

const OPTION_META_KEY = 'ttm.optionMeta';
const BELL_SOUND_KEY = 'ttm.bellSound';

type Listener = () => void;
const listeners = new Set<Listener>();

function read(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    if (v !== null) return v === '1';
  } catch {
    /* storage unavailable (private mode): use the default */
  }
  return fallback;
}

function write(key: string, on: boolean) {
  try {
    localStorage.setItem(key, on ? '1' : '0');
  } catch {
    /* applies for this page view only */
  }
  for (const l of listeners) l();
}

/** Option acts as Meta (Alt+B/F word jumps, Option+Enter) instead of typing special characters. */
export const getOptionMeta = () => read(OPTION_META_KEY, true);
export const setOptionMeta = (on: boolean) => write(OPTION_META_KEY, on);

/** A bell from an unfocused pane plays a soft sound. */
export const getBellSound = () => read(BELL_SOUND_KEY, true);
export const setBellSound = (on: boolean) => write(BELL_SOUND_KEY, on);

/** Run `fn` whenever a preference changes in this page. Returns the unsubscribe. */
export function onPrefsChange(fn: Listener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

// ---- terminal appearance --------------------------------------------------------------------

function readText(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function writeText(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* applies for this page view only */
  }
  for (const l of listeners) l();
}

function readNumber(key: string, fallback: number, min: number, max: number): number {
  const n = Number(readText(key, ''));
  return readText(key, '') !== '' && Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
}

export const CURSOR_STYLES = ['block', 'bar', 'underline'] as const;
export type CursorStyle = (typeof CURSOR_STYLES)[number];

/** The terminal's font: an id from term-fonts.ts ('custom' uses the family typed in `termCustomFont`). */
export const getTermFont = () => readText('ttm.termFont', 'jetbrains');
export const setTermFont = (id: string) => writeText('ttm.termFont', id);
export const getTermCustomFont = () => readText('ttm.termCustomFont', '');
export const setTermCustomFont = (family: string) => writeText('ttm.termCustomFont', family);
/** Draw `->` `!=` `>=` and friends as the font's ligatures (only for fonts that have them). */
export const getLigatures = () => read('ttm.ligatures', false);
export const setLigatures = (on: boolean) => write('ttm.ligatures', on);

export const LINE_HEIGHT = { min: 1, max: 1.5, step: 0.05, fallback: 1 };
export const getLineHeight = () => readNumber('ttm.lineHeight', LINE_HEIGHT.fallback, LINE_HEIGHT.min, LINE_HEIGHT.max);
export const setLineHeight = (v: number) => writeText('ttm.lineHeight', String(Math.round(v * 100) / 100));

export const LETTER_SPACING = { min: 0, max: 3, step: 1, fallback: 0 };
export const getLetterSpacing = () => readNumber('ttm.letterSpacing', LETTER_SPACING.fallback, LETTER_SPACING.min, LETTER_SPACING.max);
export const setLetterSpacing = (v: number) => writeText('ttm.letterSpacing', String(v));

export const getCursorStyle = (): CursorStyle => {
  const v = readText('ttm.cursorStyle', 'block');
  return (CURSOR_STYLES as readonly string[]).includes(v) ? (v as CursorStyle) : 'block';
};
export const setCursorStyle = (s: CursorStyle) => writeText('ttm.cursorStyle', s);
export const getCursorBlink = () => read('ttm.cursorBlink', true);
export const setCursorBlink = (on: boolean) => write('ttm.cursorBlink', on);

/** Minimum contrast ratio between text and its background (1 = leave colours alone, 4.5 = WCAG AA). */
export const CONTRAST_LEVELS = [
  { value: 1, label: 'Off' },
  { value: 3, label: 'Mild' },
  { value: 4.5, label: 'Strong' },
  { value: 7, label: 'Maximum' },
] as const;
export const getMinContrast = () => {
  const v = readNumber('ttm.minContrast', 3, 1, 21);
  return CONTRAST_LEVELS.some((c) => c.value === v) ? v : 3;
};
export const setMinContrast = (v: number) => writeText('ttm.minContrast', String(v));
/** Bold text in the brighter palette colours, as classic terminals do. */
export const getBoldBright = () => read('ttm.boldBright', true);
export const setBoldBright = (on: boolean) => write('ttm.boldBright', on);
