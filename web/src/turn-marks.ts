// Which rows of the terminal are one of your messages, so a bar can be drawn beside them. This is
// the same rule the server uses to place a message (src/turns.rs): your text, with only a few
// symbols before it on its line (`> `, `❯ `, `│ > `), so the agent quoting you does not count.
// It is a visual hint: a miss or a false mark costs nothing.

const MAX_LEAD_SYMBOLS = 4;

export function looksLikePrompt(lead: string): boolean {
  const chars = Array.from(lead);
  return chars.every((c) => !/[\p{L}\p{N}]/u.test(c)) && chars.filter((c) => !/\s/.test(c)).length <= MAX_LEAD_SYMBOLS;
}

/** Lower-case ASCII letters only, as tmux does for its search. */
const fold = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());

/** A message of yours: its id (the chat's id for it) and the first line to search for. */
export interface TurnRef {
  id: string;
  snippet: string;
}

/** The rows (0-based) of `rows` that start one of your messages, and the snippet each one matched. */
function matchRows(rows: string[], snippets: string[]): { row: number; snippet: string }[] {
  const wanted = [...new Set(snippets)].map((s) => ({ s, exact: /[A-Z]/.test(s) }));
  const out: { row: number; snippet: string }[] = [];
  rows.forEach((raw, i) => {
    const text = raw.trimEnd();
    if (!text) return;
    const lower = fold(text);
    const found = wanted.find(({ s, exact }) => {
      const hay = exact ? text : lower;
      const needle = exact ? s : fold(s);
      for (let at = hay.indexOf(needle); at >= 0; at = hay.indexOf(needle, at + 1)) {
        if (looksLikePrompt(text.slice(0, at))) return true;
      }
      return false;
    });
    if (found) out.push({ row: i, snippet: found.s });
  });
  return out;
}

/** The rows (0-based) of `rows` that start one of your messages. `snippets` are the first lines searched for. */
export function markedRows(rows: string[], snippets: string[]): number[] {
  return matchRows(rows, snippets).map((m) => m.row);
}

/**
 * The same rows, each with the message it is. `turns` come newest first. When one text was sent more
 * than once ("yes"), the lowest row on screen is taken to be the newest of them, the next up the one
 * before, and so on: right unless the screen is scrolled back into older history.
 */
export function markedTurns(rows: string[], turns: TurnRef[]): { row: number; id: string }[] {
  const byText = new Map<string, string[]>();
  for (const t of turns) byText.set(t.snippet, [...(byText.get(t.snippet) ?? []), t.id]);
  const used = new Map<string, number>();
  const out: { row: number; id: string }[] = [];
  for (const m of matchRows(rows, turns.map((t) => t.snippet)).reverse()) {
    const k = used.get(m.snippet) ?? 0;
    used.set(m.snippet, k + 1);
    const id = byText.get(m.snippet)?.[k];
    if (id !== undefined) out.push({ row: m.row, id });
  }
  return out.reverse();
}
