// The command bar's data and ranking. No DOM, so it is unit-testable.

/** What choosing a command does: run something, or open a second step (a folder to pick, a path to type). */
export interface Item {
  id: string;
  label: string;
  hint?: string;
  /** Shown as a small tag and matched by the search: Sessions, Agent, Terminal… */
  group?: string;
  /** Extra words the search should match, so "kill" finds Close. */
  keywords?: string;
  /** Tab puts this in the input (used to complete a folder path). */
  complete?: string;
  run?: () => void;
  step?: () => Step;
}

/** One screen of the bar: a prompt and the items for what has been typed. */
export interface Step {
  placeholder: string;
  items: (query: string) => Item[] | Promise<Item[]>;
  /** The items already fit the query (the server filtered them): show them as given. */
  filtered?: boolean;
}

/** How well `query` matches `text`, or -1. Every space-separated word must match somewhere. */
export function score(query: string, text: string): number {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const hay = text.toLowerCase();
  let total = 0;
  for (const w of words) {
    const at = hay.indexOf(w);
    if (at >= 0) {
      const wordStart = at === 0 || /[^a-z0-9]/.test(hay[at - 1]!);
      total += wordStart ? 100 - Math.min(at, 40) : 50;
      continue;
    }
    // Letters in order but close together ("nwp" finds "New pane"); spread across a long label, no.
    let i = 0;
    let first = -1;
    let last = -1;
    for (let k = 0; k < hay.length && i < w.length; k++) {
      if (hay[k] !== w[i]) continue;
      if (first < 0) first = k;
      last = k;
      i++;
    }
    if (i < w.length || last - first + 1 > w.length * 2) return -1;
    total += 15;
  }
  return total;
}

const RECENT_BONUS = 25;

/** The items that match `query`, best first. With nothing typed, recent ones come first, in order of use. */
export function rank(items: Item[], query: string, recent: string[]): Item[] {
  const used = (id: string) => recent.indexOf(id);
  if (!query.trim()) {
    return items
      .map((it, i) => ({ it, i, r: used(it.id) }))
      .sort((a, b) => (a.r < 0 ? 1e6 : a.r) - (b.r < 0 ? 1e6 : b.r) || a.i - b.i)
      .map((x) => x.it);
  }
  return items
    .map((it, i) => ({ it, i, s: score(query, `${it.label} ${it.keywords ?? ''} ${it.group ?? ''}`) }))
    .filter((x) => x.s >= 0)
    .map((x) => ({ ...x, s: x.s + (used(x.it.id) >= 0 ? RECENT_BONUS : 0) }))
    .sort((a, b) => b.s - a.s || a.i - b.i)
    .map((x) => x.it);
}

/** `recent` with `id` moved to the front, at most `max` long. */
export function remember(recent: string[], id: string, max = 8): string[] {
  return [id, ...recent.filter((r) => r !== id)].slice(0, max);
}
