// What the server says about newer releases, and what to show for it. Pure, so node can test it;
// server-update.ts does the drawing and update.ts the watching.

export interface ReleaseInfo {
  version: string;
  title: string;
  notes: string;
  url: string;
  publishedAt: string;
}

export interface LastResult {
  ok: boolean;
  from: string;
  to: string;
  /** Unix seconds. */
  at: number;
}

export interface ServerUpdate {
  current: string;
  latest: string | null;
  /** Newer than `current`, newest first. */
  releases: ReleaseInfo[];
  /** Unix seconds of the last look at GitHub. */
  checkedAt: number | null;
  error: string | null;
  canInstall: boolean;
  cannotInstallReason: string | null;
  installing: boolean;
  lastResult: LastResult | null;
}

/** A release title is "v0.3.0 — what it is about"; the part after the dash. */
export const headline = (r: ReleaseInfo): string => r.title.replace(/^v?\d+\.\d+\.\d+\s*([—–-]\s*)?/, '').trim();

/** The toast for the newest release: "ttm v0.3.0 — Fix a sheet that would not open twice". */
export function offerText(u: ServerUpdate): string | null {
  const r = u.releases[0];
  if (!r) return null;
  const what = headline(r);
  return `ttm v${r.version}${what ? ` — ${what}` : ''}`;
}

/** Offer a version once per page life: a dismissed toast stays dismissed until the next launch. */
export const shouldOffer = (u: ServerUpdate, offered: string | null): boolean => u.latest !== null && u.latest !== offered;

/** Whether an update that failed and was rolled back is news, given the time we last told the user. */
export const failureNews = (last: LastResult | null, seenAt: number): string | null =>
  last && !last.ok && last.at > seenAt ? `Update to ttm v${last.to} did not start, so v${last.from} was restored.` : null;

/** After an install: has the restarted server come back as the version we asked for? */
export const arrived = (u: ServerUpdate, target: string): boolean => u.current === target;

export function checkedAgo(checkedAt: number | null, nowSecs: number): string {
  if (checkedAt === null) return 'not checked yet';
  const s = Math.max(0, nowSecs - checkedAt);
  if (s < 60) return 'checked just now';
  if (s < 3600) return `checked ${Math.floor(s / 60)} min ago`;
  return `checked ${Math.floor(s / 3600)} h ago`;
}
