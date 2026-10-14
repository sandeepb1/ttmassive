// Which build a page is, and whether it is behind. Pure, so node can test it; update.ts does the watching.

/** The build a page names: the hashed entry script in its HTML. Null for a dev server, which has none. */
export function buildOf(html: string): string | null {
  return html.match(/<script[^>]+type="module"[^>]*src="(\/assets\/[^"]+\.js)"/)?.[1] ?? html.match(/src="(\/assets\/[^"]+\.js)"[^>]*type="module"/)?.[1] ?? null;
}

/** Whether the running build is not the latest. `latest` is null when the latest could not be read. */
export const isStale = (running: string | null, latest: string | null) => running !== null && latest !== null && running !== latest;
