// History search: what the server sends back and the small decisions the search bar makes from it.
// Pure, so node can test it; `term-search.ts` does the DOM.

export interface Hit {
  /** This match counted from the newest, which is 1. The jump takes this number. */
  n: number;
  /** Lines between this one and the last line with text. */
  up: number;
  /** The line, cut around the match. */
  text: string;
  /** Where the match starts and how long it is, in characters of `text`. */
  at: number;
  len: number;
}

export interface Found {
  total: number;
  /** There are more matches than `total`: counting stopped. */
  capped: boolean;
  hits: Hit[];
}

export type Step = 'older' | 'newer';

const isNum = (x: unknown): x is number => typeof x === 'number' && Number.isFinite(x);

/** Read a server reply, dropping anything malformed. */
export function parseFound(raw: unknown): Found {
  const none: Found = { total: 0, capped: false, hits: [] };
  if (typeof raw !== 'object' || raw === null) return none;
  const r = raw as Record<string, unknown>;
  const hits: Hit[] = [];
  if (Array.isArray(r.hits)) {
    for (const h of r.hits as Record<string, unknown>[]) {
      if (h && isNum(h.n) && isNum(h.up) && typeof h.text === 'string' && isNum(h.at) && isNum(h.len)) {
        hits.push({ n: h.n, up: h.up, text: h.text, at: h.at, len: h.len });
      }
    }
  }
  return { total: isNum(r.total) ? Math.max(0, r.total) : hits.length, capped: r.capped === true, hits };
}

/** The match to go to when stepping from `current` (none yet) in a direction, or null when there are none. */
export function stepTo(current: number | null, dir: Step, total: number): number | null {
  if (total < 1) return null;
  if (current === null) return 1;
  return dir === 'older' ? Math.min(total, current + 1) : Math.max(1, current - 1);
}

export function counter(current: number | null, total: number, capped: boolean): string {
  if (total < 1) return 'No matches';
  const all = `${total}${capped ? '+' : ''}`;
  return current === null ? `${all} ${total === 1 ? 'match' : 'matches'}` : `${current} of ${all}`;
}

export function upLabel(up: number): string {
  return up === 0 ? 'last line' : `${up} line${up === 1 ? '' : 's'} up`;
}

/** A hit's line as text before the match, the match, and text after it. */
export function parts(h: Hit): [string, string, string] {
  const chars = Array.from(h.text);
  return [chars.slice(0, h.at).join(''), chars.slice(h.at, h.at + h.len).join(''), chars.slice(h.at + h.len).join('')];
}

// ---------------------------------------------------------------- your messages (turns)

/** One of your messages to the agent, and where it is in the terminal history, if it is. */
export interface Turn {
  id: string;
  /** Unix seconds, when the transcript says. */
  ts: number | null;
  preview: string;
  /** What to search for to reach it: sent as `q` with the jump. */
  snippet: string;
  /** Its match number from the newest, or null when it is not on the terminal any more. */
  n: number | null;
  /** The line it is on, sent back with the jump. */
  text: string | null;
  up: number | null;
}

export function parseTurns(raw: unknown): Turn[] {
  const list = typeof raw === 'object' && raw !== null ? (raw as { turns?: unknown }).turns : null;
  if (!Array.isArray(list)) return [];
  const out: Turn[] = [];
  for (const t of list as Record<string, unknown>[]) {
    if (!t || typeof t.id !== 'string' || typeof t.preview !== 'string' || typeof t.snippet !== 'string') continue;
    const placed = isNum(t.n) && typeof t.text === 'string';
    out.push({
      id: t.id,
      ts: isNum(t.ts) ? t.ts : null,
      preview: t.preview,
      snippet: t.snippet,
      n: placed ? (t.n as number) : null,
      text: placed ? (t.text as string) : null,
      up: placed && isNum(t.up) ? t.up : null,
    });
  }
  return out;
}

/**
 * The message to go to when stepping from `current` (an index into `turns`, newest first). Older
 * is further down the list. Messages that are not on the terminal are skipped.
 */
export function stepTurn(turns: Turn[], current: number | null, dir: Step): number | null {
  const step = dir === 'older' ? 1 : -1;
  for (let i = current === null ? (dir === 'older' ? 0 : turns.length - 1) : current + step; i >= 0 && i < turns.length; i += step) {
    if (turns[i]!.n !== null) return i;
  }
  return null;
}

export function turnCounter(turns: Turn[], current: number | null): string {
  const here = turns.filter((t) => t.n !== null).length;
  if (turns.length === 0) return '';
  if (current === null) return `${turns.length} message${turns.length === 1 ? '' : 's'}`;
  const pos = turns.slice(0, current + 1).filter((t) => t.n !== null).length;
  return `${pos} of ${here}`;
}

/** "5 min ago", for the list. */
export function ago(ts: number | null, nowSecs: number): string {
  if (ts === null) return '';
  const s = Math.max(0, nowSecs - ts);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}
