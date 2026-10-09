// How the session list is organised: pinned sessions first, then one section per working folder.
// Pure, so node can test it.

import { splitPath } from './chat-insights.ts';

/** The fields of a session this module needs. */
export interface Groupable {
  id: string;
  pinned: boolean;
  cwd: string | null;
  status: 'running' | 'exited' | 'stopped';
  idle_secs: number;
  activity: 'working' | 'waiting' | 'idle' | 'unknown' | null;
}

export interface Section<T> {
  /** Stable id for remembering collapsed state. */
  key: string;
  title: string;
  /** The folder's parent, to tell two folders of the same name apart. */
  sub: string;
  items: T[];
}

export const PINNED_KEY = '__pinned';
export const NO_FOLDER_KEY = '__none';

/**
 * Sections for the list: Pinned, then folders in the order their newest session appears (the list
 * arrives newest first). Sessions with no known folder share one section.
 */
export function groupSessions<T extends Groupable>(sessions: T[]): Section<T>[] {
  const pinned = sessions.filter((s) => s.pinned);
  const out: Section<T>[] = [];
  if (pinned.length > 0) out.push({ key: PINNED_KEY, title: 'Pinned', sub: '', items: pinned });
  const byFolder = new Map<string, Section<T>>();
  for (const s of sessions) {
    if (s.pinned) continue;
    const key = s.cwd ?? NO_FOLDER_KEY;
    let sec = byFolder.get(key);
    if (!sec) {
      const { name, dir } = s.cwd ? splitPath(s.cwd) : { name: 'No folder', dir: '' };
      sec = { key, title: name, sub: dir, items: [] };
      byFolder.set(key, sec);
      out.push(sec);
    }
    sec.items.push(s);
  }
  return out;
}

export type DotState = 'stopped' | 'exited' | 'waiting' | 'working' | 'ready' | 'idle';

/** The colour of a session's dot: what it is doing, with a fallback to recent output. */
export function dotState(s: Groupable, activeSecs = 5): DotState {
  if (s.status === 'stopped') return 'stopped';
  if (s.status === 'exited') return 'exited';
  switch (s.activity) {
    case 'waiting':
      return 'waiting';
    case 'working':
      return 'working';
    case 'idle':
      return 'ready'; // an agent at its prompt, ready for the next instruction
    default:
      return s.idle_secs < activeSecs ? 'working' : 'idle';
  }
}

export const DOT_LABEL: Record<DotState, string> = {
  stopped: 'stopped',
  exited: 'exited',
  waiting: 'waiting for you',
  working: 'working',
  ready: 'ready',
  idle: 'idle',
};

/**
 * Two characters to tell a session apart in the narrow rail: the initials of a hyphenated name
 * (`claude-cedarwing` is CC), otherwise its first two letters (`docs` is Do).
 */
export function railLabel(id: string): string {
  const parts = id.split(/[-_.]+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0]![0]! + parts[1]![0]!).toUpperCase();
  const word = parts[0] ?? id;
  return word.slice(0, 1).toUpperCase() + word.slice(1, 2).toLowerCase();
}
