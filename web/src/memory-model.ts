// What the Memory tab shows, as the server sends it, and the small pieces of logic that turn it
// into words. No DOM here, so it can be tested.

export interface Job {
  running: boolean;
  started: number;
  finished: number;
  error: string | null;
}

export interface DaySummary {
  date: string;
  entries: number;
  bytes: number;
}

export interface MemoryOverview {
  root: string;
  collection: string;
  days: DaySummary[];
  entries: number;
  chunks: number | null;
  index_status: string | null;
  last_indexed: string | null;
  /** A note is newer than the last successful index run. */
  unindexed: boolean;
  provider: string | null;
  model: string | null;
  llm_model: string | null;
  reindex: Job;
  rollup: Job;
}

export interface Entry {
  time: string;
  group: string | null;
  session: string | null;
  bullets: string[];
}

export interface Hit {
  chunk_hash: string;
  date: string;
  heading: string;
  score: number;
  text: string;
  session: string | null;
  start_line: number;
  end_line: number;
}

export interface RollupItem {
  text: string;
  date: string | null;
}

export interface RollupInfo {
  generated_at: number;
  model: string;
  stale: boolean;
  decisions: RollupItem[];
  open_questions: RollupItem[];
  topics: { name: string; summary: string }[];
}

export interface RollupView {
  job: Job;
  rollup: RollupInfo | null;
}

const DAY_MS = 86_400_000;
const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** `2026-10-14` as `Today`, `Yesterday`, `Mon 12 Oct`, or the date itself if it is not one. */
export function dayLabel(date: string, today: Date): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!m) return date;
  const day = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  if (Number.isNaN(day.getTime())) return date;
  const midnight = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const diff = Math.round((midnight.getTime() - day.getTime()) / DAY_MS);
  if (diff === 0) return 'Today';
  if (diff === 1) return 'Yesterday';
  const year = day.getFullYear() === today.getFullYear() ? '' : ` ${day.getFullYear()}`;
  return `${WEEKDAYS[day.getDay()]} ${day.getDate()} ${MONTHS[day.getMonth()]}${year}`;
}

/** A relevance score (0 to 1) as a percentage. */
export function scoreLabel(score: number): string {
  return `${Math.round(Math.min(1, Math.max(0, score)) * 100)}%`;
}

/** `~/code/app` for a path under the home folder `/Users/x/code/app`, else the last two folders. */
export function shortPath(path: string): string {
  const home = /^\/(?:Users|home)\/[^/]+(\/.*)?$/.exec(path);
  if (home) return `~${home[1] ?? ''}`;
  const parts = path.split('/').filter(Boolean);
  return parts.length > 2 ? `…/${parts.slice(-2).join('/')}` : path;
}

/** A search hit's text as a short preview: the `### 12:20` heading is dropped, bullets are joined. */
export function preview(text: string, max = 240): string {
  const body = text
    .split('\n')
    .filter((l) => !/^#{1,6}\s/.test(l))
    .map((l) => l.replace(/^\s*-\s+/, '').trim())
    .filter(Boolean)
    .join(' · ');
  return body.length > max ? `${body.slice(0, max).trimEnd()}…` : body;
}

/** The state of a background job as a line to show, or empty when there is nothing to say. */
export function jobLine(job: Job, doing: string, failed: string): string {
  if (job.running) return doing;
  return job.error ? `${failed}: ${job.error}` : '';
}

/** `3m ago` for an RFC 3339 timestamp, or `never` when there is none. */
export function indexedAgo(iso: string | null, nowMs: number): string {
  if (!iso) return 'never';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return 'unknown';
  const s = Math.max(0, Math.round((nowMs - t) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m ago`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86_400)}d ago`;
}

/** Entries as `3 entries`, `1 entry`. */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
