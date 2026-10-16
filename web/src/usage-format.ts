// Plan-limit usage: the shapes `/api/usage` returns and the pure helpers that turn them into
// text. No DOM here, so node can test it.

export interface LimitWindow {
  /** 0-100. */
  used_pct: number;
  /** Unix seconds. */
  resets_at: number | null;
}

export interface ModelWindow extends LimitWindow {
  name: string;
}

export interface ToolUsage {
  five_hour: LimitWindow | null;
  seven_day: LimitWindow | null;
  per_model: ModelWindow[];
  plan: string | null;
  /** Unix seconds; 0 when nothing has been read yet. */
  fetched_at: number;
  source: 'api' | 'hook' | 'session_log' | 'team_claude';
  /** Why the latest refresh failed. Older numbers may still be present. */
  error: string | null;
}

export interface SessionTokens {
  tool: 'claude' | 'codex';
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

/** What one OpenRouter key has spent and has left, in dollars. */
export interface OrUsage {
  id: string;
  name: string;
  spent: number | null;
  daily: number | null;
  monthly: number | null;
  /** The key's credit limit and what is left; null for a key with no limit. */
  limit: number | null;
  remaining: number | null;
  error: string | null;
  fetchedAt: number;
}

export interface UsageSnapshot {
  claude: ToolUsage;
  codex: ToolUsage;
  sessions: Record<string, SessionTokens>;
  /** One per OpenRouter account with a key; absent from an older server. */
  openrouter?: OrUsage[];
}

export type Level = 'ok' | 'warn' | 'crit';

export const WARN_PCT = 75;
export const CRIT_PCT = 90;
/** Numbers older than this are shown dimmed. */
export const STALE_SECS = 30 * 60;

export function level(pct: number): Level {
  if (pct >= CRIT_PCT) return 'crit';
  if (pct >= WARN_PCT) return 'warn';
  return 'ok';
}

/** Time until a reset: `now`, `42m`, `2h10m`, `3d4h`; empty when unknown. */
export function countdown(resetsAt: number | null, now: number): string {
  if (resetsAt === null) return '';
  const s = resetsAt - now;
  if (s <= 0) return 'now';
  if (s < 3600) return `${Math.ceil(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}m`;
  return `${Math.floor(s / 86400)}d${Math.floor((s % 86400) / 3600)}h`;
}

/** How long ago numbers were read: `just now`, `3m ago`, `2h ago`; `never` for 0. */
export function ago(fetchedAt: number, now: number): string {
  if (fetchedAt <= 0) return 'never';
  const s = Math.max(0, now - fetchedAt);
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function isStale(u: ToolUsage, now: number): boolean {
  return u.fetched_at <= 0 || now - u.fetched_at > STALE_SECS;
}

/** `950`, `1.2k`, `12k`, `3.4M`. */
export function compact(n: number): string {
  if (n < 1000) return String(n);
  const fmt = (v: number) => (v < 10 ? v.toFixed(1).replace(/\.0$/, '') : String(Math.round(v)));
  // 999_999 would round to "1000k": call that 1M instead.
  if (Math.round(n / 1e3) < 1000) return `${fmt(n / 1e3)}k`;
  return `${fmt(n / 1e6)}M`;
}

/** `12k in · 4.1k out · 1.2M cached`: what a session has used. */
export function tokenLine(t: SessionTokens): string {
  const parts = [`${compact(t.input + t.cache_write)} in`, `${compact(t.output)} out`];
  if (t.cache_read > 0) parts.push(`${compact(t.cache_read)} cached`);
  return parts.join(' · ');
}

export interface Segment {
  text: string;
  level: Level;
}

/** The strip's numbers for one tool. `unavailable` when there is nothing to show. */
export function stripSegments(u: ToolUsage): { items: Segment[]; unavailable: boolean } {
  const items: Segment[] = [];
  for (const [label, w] of [['5h', u.five_hour], ['7d', u.seven_day]] as const) {
    if (w) items.push({ text: `${label} ${Math.round(w.used_pct)}%`, level: level(w.used_pct) });
  }
  return { items, unavailable: items.length === 0 };
}

export const SOURCE_LABEL: Record<ToolUsage['source'], string> = {
  api: 'live',
  hook: 'from claude',
  session_log: 'from codex log',
  team_claude: 'Team Claude pool',
};

/** Dollars: cents when it is a few, whole dollars when it is many. */
export function dollars(n: number): string {
  if (n >= 1000) return `$${Math.round(n)}`;
  if (n >= 100) return `$${n.toFixed(0)}`;
  if (n >= 0.1) return `$${n.toFixed(2)}`;
  return n > 0 ? `$${n.toFixed(3)}` : '$0';
}

/** How much of a key's limit is used, 0 to 100, or null when it has no limit. */
export function orUsedPct(u: OrUsage): number | null {
  if (u.limit === null || u.limit <= 0) return null;
  const left = u.remaining ?? u.limit - (u.spent ?? 0);
  return Math.min(100, Math.max(0, ((u.limit - left) / u.limit) * 100));
}

/** The strip's one figure for a key: what is left when it has a limit, else what was spent today (or in all). */
export function orStrip(u: OrUsage): { text: string; level: Level } {
  if (u.error) return { text: '⚠', level: 'warn' };
  const pct = orUsedPct(u);
  if (pct !== null && u.remaining !== null) return { text: `${dollars(u.remaining)} left`, level: level(pct) };
  if (u.daily !== null) return { text: `${dollars(u.daily)} today`, level: 'ok' };
  if (u.spent !== null) return { text: `${dollars(u.spent)} spent`, level: 'ok' };
  return { text: '–', level: 'ok' };
}

/** The lines the panel shows for a key. */
export function orLines(u: OrUsage): string[] {
  const out: string[] = [];
  if (u.limit !== null && u.remaining !== null) out.push(`${dollars(u.remaining)} left of ${dollars(u.limit)}`);
  if (u.daily !== null) out.push(`${dollars(u.daily)} today`);
  if (u.monthly !== null) out.push(`${dollars(u.monthly)} this month`);
  if (u.spent !== null) out.push(`${dollars(u.spent)} in all`);
  return out;
}
