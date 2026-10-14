// The corner chip on a terminal pane: what the agent is doing and for how long. Pure, so node can
// test it; `term-chip.ts` draws it.
//
// Three sources feed it. The screen reader says whether the agent is working, waiting for you or
// idle (it already runs every two seconds). The conversation says what it is doing (the to-do it
// is on, or the tool it is running) and when your last message was sent. The page's own clock
// times what it has watched itself.

import { fmtElapsed } from './agents.ts';
import type { ChatItem, Todo } from './chat-model';

export type Activity = 'working' | 'waiting' | 'idle' | 'unknown';

export interface Tracker {
  activity: 'working' | 'waiting' | 'idle';
  /** Page time (ms) the activity was first seen. */
  since: number;
  /** The page saw the change into this activity, rather than arriving with it already under way. */
  sawStart: boolean;
  /** Page time (ms) the screen first became unreadable, while it stays so. */
  unreadableSince?: number;
}

export interface Work {
  /** What the agent is doing right now, if its records say. */
  doing: string | null;
  /** Unix seconds when you sent the message it is answering. */
  turnStart: number | null;
}

export interface Chip {
  kind: 'working' | 'waiting';
  label: string;
  detail: string;
  secs: number;
}

const DOING_CHARS = 56;
/** A turn that began longer ago than this is taken as stale, not as the one under way. */
const MAX_TURN_SECS = 12 * 3600;

function clip(s: string): string {
  const one = s.replace(/\s+/g, ' ').trim();
  return one.length > DOING_CHARS ? `${one.slice(0, DOING_CHARS - 1)}…` : one;
}

/** What the agent is doing now: the to-do it is on, else the tool it is running in this turn. */
export function doingNow(todos: Todo[], items: ChatItem[]): string | null {
  const todo = todos.find((t) => t.status === 'in_progress');
  if (todo) return clip(todo.active ?? todo.content) || null;
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i]!;
    if (it.kind === 'user') break; // before your message is a different turn
    if (it.kind === 'tool' && it.status === 'running') return clip(`${it.tool} ${it.summary}`) || null;
  }
  return null;
}

/** When your latest message was sent, in unix seconds. */
export function lastUserTs(items: ChatItem[]): number | null {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i]!;
    if (it.kind === 'user') return it.ts;
  }
  return null;
}

/** How long an unreadable screen is taken as a blip before it is taken as "nothing going on". */
export const UNREADABLE_GRACE_MS = 10_000;

/**
 * Note what the screen reader says now. A screen it cannot read for a moment changes nothing: it
 * must not reset the clock in the middle of a long turn. One it cannot read for long (the agent
 * shows neither a spinner nor a prompt) means there is nothing to report, so the chip must not
 * stay on "Working" for ever.
 */
export function observe(prev: Tracker | null, activity: Activity, nowMs: number): Tracker {
  if (activity === 'unknown') {
    if (!prev) return { activity: 'idle', since: nowMs, sawStart: true };
    const since = prev.unreadableSince ?? nowMs;
    if (nowMs - since >= UNREADABLE_GRACE_MS) return { activity: 'idle', since: nowMs, sawStart: true };
    return prev.unreadableSince === since ? prev : { ...prev, unreadableSince: since };
  }
  if (prev && prev.activity === activity) return prev.unreadableSince === undefined ? prev : { activity, since: prev.since, sawStart: prev.sawStart };
  return { activity, since: nowMs, sawStart: prev !== null };
}

/** How far the server's clock is ahead of this page's, from a response's `Date` header. */
export function skewFromDate(header: string | null, nowMs: number): number {
  const t = header ? Date.parse(header) : NaN;
  return Number.isFinite(t) ? t - nowMs : 0;
}

/** The chip to show, or null when there is nothing to say (idle). */
export function chip(t: Tracker | null, work: Work, skewMs: number, nowMs: number): Chip | null {
  if (!t || t.activity === 'idle') return null;
  const seen = Math.max(0, (nowMs - t.since) / 1000);
  if (t.activity === 'waiting') return { kind: 'waiting', label: 'Waiting for you', detail: '', secs: seen };
  // Arriving mid-turn, the page has no start of its own to go by: use when you sent the message.
  // Having watched it begin, trust that: the conversation may not have caught up yet.
  let secs = seen;
  if (!t.sawStart && work.turnStart !== null) {
    const turn = (nowMs + skewMs) / 1000 - work.turnStart;
    if (turn >= 0 && turn <= MAX_TURN_SECS) secs = turn;
  }
  return { kind: 'working', label: 'Working', detail: work.doing ?? '', secs };
}

export function chipText(c: Chip): string {
  return [c.label, c.detail, fmtElapsed(c.secs)].filter(Boolean).join(' · ');
}
