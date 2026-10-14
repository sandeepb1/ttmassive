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

/** The rows (0-based) of `rows` that start one of your messages. `snippets` are the first lines searched for. */
export function markedRows(rows: string[], snippets: string[]): number[] {
  const wanted = [...new Set(snippets)].map((s) => ({ s, exact: /[A-Z]/.test(s) }));
  const out: number[] = [];
  rows.forEach((raw, i) => {
    const text = raw.trimEnd();
    if (!text) return;
    const lower = fold(text);
    const found = wanted.some(({ s, exact }) => {
      const hay = exact ? text : lower;
      const needle = exact ? s : fold(s);
      for (let at = hay.indexOf(needle); at >= 0; at = hay.indexOf(needle, at + 1)) {
        if (looksLikePrompt(text.slice(0, at))) return true;
      }
      return false;
    });
    if (found) out.push(i);
  });
  return out;
}
