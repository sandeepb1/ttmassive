// What the Team Claude tab shows, as the server passes it on from the proxy, and the logic that
// reads it. No DOM here, so it can be tested. Every field of the proxy's JSON is optional: it is
// someone else's format, and a field that goes missing should blank a line, not break the tab.

export type Provider = 'anthropic' | 'codex';

export interface TcAccount {
  name: string;
  provider?: string;
  type?: string;
  status?: string;
  disabled?: boolean;
  priority?: number;
  sessions?: number;
  orgName?: string | null;
  rateLimitedUntil?: number | string | null;
  pausedUntil?: number | string | null;
  quota?: {
    unified5h?: number | null;
    unified7d?: number | null;
    unified5hReset?: number | null;
    unified7dReset?: number | null;
  };
  usage?: {
    totalRequests?: number;
    totalInputTokens?: number;
    totalOutputTokens?: number;
    lastUsed?: string | null;
    recentModels?: Record<string, number | string>;
  };
}

export interface TcStatus {
  server?: {
    version?: string;
    uptimeSeconds?: number;
    upstream?: string;
    eventLoop?: { lastLagMs?: number; stallCount?: number };
  };
  accounts?: TcAccount[];
  currentAccounts?: Record<string, string>;
  defaultTargets?: Record<string, string>;
  switchThreshold?: number | null;
  expiryRouting?: { enabled?: boolean };
  routes?: unknown[];
  sessions?: { active?: number };
}

export interface PoolBucket {
  utilization: number | null;
  remaining?: number | null;
  knownAccounts?: number;
  nextResetAt: number | null;
}

export interface TcQuota {
  aggregate?: Partial<Record<'fiveHour' | 'weeklyShared' | 'weeklySonnet' | 'weeklyFable', PoolBucket>>;
}

/** `GET /api/sessions/{id}/teamclaude`. */
export interface TcReply {
  detected: boolean;
  agent: 'claude' | 'codex' | 'other' | null;
  error: string | null;
  origin?: string;
  provider?: Provider;
  loopback?: boolean;
  dashboard?: string;
  status?: TcStatus;
  quota?: TcQuota | null;
}

export interface TcRequest {
  at: number;
  account: string;
  retry: number;
  method: string;
  path: string;
  status: number | null;
  error: boolean;
  mine: boolean;
}

export interface TcFeed {
  enabled: boolean;
  level: string | null;
  requests: TcRequest[];
}

export const providerOf = (agent: TcReply['agent']): Provider | null =>
  agent === 'claude' ? 'anthropic' : agent === 'codex' ? 'codex' : null;

export const providerName = (p: string | undefined): string =>
  p === 'anthropic' ? 'Claude' : p === 'codex' ? 'Codex' : (p ?? 'Unknown');

/** A time the proxy gave as milliseconds or as text, in milliseconds; null if it is neither. */
export function toMs(v: number | string | null | undefined): number | null {
  if (typeof v === 'number') return Number.isFinite(v) ? v : null;
  if (typeof v === 'string') {
    const t = Date.parse(v);
    return Number.isNaN(t) ? null : t;
  }
  return null;
}

/** The accounts of one provider, the active one first, then by priority and name. */
export function accountsFor(status: TcStatus | undefined, provider: string): TcAccount[] {
  const active = activeAccount(status, provider);
  return (status?.accounts ?? [])
    .filter((a) => a.provider === provider)
    .sort((a, b) => Number(b.name === active) - Number(a.name === active) || (a.priority ?? 0) - (b.priority ?? 0) || a.name.localeCompare(b.name));
}

/** The account that takes the next request for `provider`. */
export function activeAccount(status: TcStatus | undefined, provider: string): string | null {
  return status?.currentAccounts?.[provider] ?? null;
}

/**
 * Whether the proxy can switch `provider`'s traffic to `name` for us. It finds an account by name
 * and takes the first with that name, so a name an earlier account of another provider also has
 * cannot be reached. The server enforces this too; knowing it here saves offering a dead button.
 */
export function canSwitchTo(status: TcStatus | undefined, provider: string, name: string): boolean {
  const norm = (s: string) => s.trim().toLowerCase();
  const first = (status?.accounts ?? []).find((a) => norm(a.name) === norm(name));
  return first?.provider === provider && activeAccount(status, provider) !== name;
}

export type AccountState = 'active' | 'ready' | 'limited' | 'paused' | 'disabled' | 'unavailable';

/** What to call an account's condition at `nowMs`. */
export function accountState(a: TcAccount, active: boolean, nowMs: number): AccountState {
  if (a.disabled) return 'disabled';
  const limited = toMs(a.rateLimitedUntil);
  if (limited !== null && limited > nowMs) return 'limited';
  const paused = toMs(a.pausedUntil);
  if (paused !== null && paused > nowMs) return 'paused';
  if (a.status && !['active', 'ok', 'ready'].includes(a.status)) return 'unavailable';
  return active ? 'active' : 'ready';
}

export const STATE_LABEL: Record<AccountState, string> = {
  active: 'In use',
  ready: 'Ready',
  limited: 'Rate limited',
  paused: 'Paused',
  disabled: 'Disabled',
  unavailable: 'Unavailable',
};

/** A fraction of a limit (0 to 1) as the 0 to 100 the meters use; null when it is unknown. */
export function pct(fraction: number | null | undefined): number | null {
  return typeof fraction === 'number' && Number.isFinite(fraction) ? Math.min(100, Math.max(0, fraction * 100)) : null;
}

/** `5d 6h`, `3h 12m`, `8m`, `42s`. */
export function uptime(seconds: number | undefined): string {
  if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '';
  const s = Math.floor(seconds);
  const d = Math.floor(s / 86_400);
  const h = Math.floor((s % 86_400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${s}s`;
}

/** 4054 as `4.1k`, 2165019 as `2.2M`. */
export function compact(n: number | undefined): string {
  if (n === undefined || !Number.isFinite(n)) return '–';
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e4) return `${Math.round(n / 1e3)}k`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(Math.round(n));
}

/** Model names by how recently the account used them, newest first, without date suffixes. */
export function recentModels(a: TcAccount, max = 3): string[] {
  const entries = Object.entries(a.usage?.recentModels ?? {})
    .map(([name, at]) => [name, toMs(at) ?? 0] as const)
    .sort((x, y) => y[1] - x[1]);
  return entries.slice(0, max).map(([name]) => name.replace(/-\d{8}$/, ''));
}

export type StatusKind = 'ok' | 'warn' | 'err';

/** The colour of a request's status code. */
export function statusKind(r: Pick<TcRequest, 'status' | 'error'>): StatusKind {
  if (r.error || r.status === null) return r.error ? 'err' : 'warn';
  if (r.status >= 500 || r.status === 429) return 'err';
  if (r.status >= 400) return 'warn';
  return 'ok';
}

/** A request's age as `12s`, `4m`, `3h`, `2d`. */
export function requestAge(at: number, nowSecs: number): string {
  const s = Math.max(0, nowSecs - at);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86_400)}d`;
}

/** Whether the page is open on the same machine as the proxy, where its dashboard can be reached. */
export function dashboardReachable(reply: TcReply, pageHost: string): boolean {
  return !!reply.dashboard && !!reply.loopback && ['localhost', '127.0.0.1', '[::1]', '::1'].includes(pageHost);
}
