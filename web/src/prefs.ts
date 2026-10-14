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
