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
