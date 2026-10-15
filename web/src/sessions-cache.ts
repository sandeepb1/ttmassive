// The last session list the server gave, kept so the app can still show your sessions when it cannot reach the
// server (a train, a tunnel that is down). Whatever it holds may be stale, so it is only ever shown with a note
// and never used to decide anything the server must decide.

import type { SessionView } from './sessions';

const KEY = 'ttm.sessions.cache';

interface Store {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const defaultStore = (): Store | null => {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
};

export function writeSessionCache(list: SessionView[], store: Store | null = defaultStore()) {
  try {
    store?.setItem(KEY, JSON.stringify(list));
  } catch {
    /* full or blocked: the next launch just has nothing to show offline */
  }
}

/** The cached list, or null when there is none or it is not usable. */
export function readSessionCache(store: Store | null = defaultStore()): SessionView[] | null {
  try {
    const raw = store?.getItem(KEY);
    if (!raw) return null;
    const v: unknown = JSON.parse(raw);
    if (!Array.isArray(v) || v.length === 0) return null;
    if (!v.every((s) => typeof s === 'object' && s !== null && typeof (s as { id?: unknown }).id === 'string')) return null;
    return v as SessionView[];
  } catch {
    return null;
  }
}
