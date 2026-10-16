// Provider accounts and the model lists behind their pickers: the shapes the server sends, and the
// sorting, filtering and formatting the pickers need. No DOM here, so it can be tested on its own.

export type AccountKind = 'openrouter' | 'openai-compatible';

/** How OpenRouter routes an account's requests. Sent to it as the request's `provider` object. */
export interface Prefs {
  /** None lets OpenRouter balance price, speed and uptime. */
  sort: 'price' | 'throughput' | 'latency' | null;
  /** Only providers that neither keep nor train on prompts. */
  denyDataCollection: boolean;
  /** Only zero-data-retention providers. */
  zdr: boolean;
  /** Try another provider when the first fails. */
  allowFallbacks: boolean;
}

export const DEFAULT_PREFS: Prefs = { sort: null, denyDataCollection: false, zdr: false, allowFallbacks: true };

export const SORT_CHOICES: { value: string; label: string }[] = [
  { value: '', label: 'Balanced' },
  { value: 'price', label: 'Cheapest' },
  { value: 'throughput', label: 'Fastest output' },
  { value: 'latency', label: 'Lowest latency' },
];

export interface PinnedModel {
  id: string;
  /** Dollars per million tokens; null when unknown. */
  priceIn: number | null;
  priceOut: number | null;
  tools: boolean;
}

export interface Account {
  id: string;
  name: string;
  kind: AccountKind;
  baseUrl: string;
  hasKey: boolean;
  last4: string;
  useInOrchestrator: boolean;
  models: PinnedModel[];
  prefs: Prefs;
}

/** What the server accepts when an account is added or changed. A missing `key` keeps the stored one. */
export interface AccountInput {
  name: string;
  kind: AccountKind;
  baseUrl: string;
  key?: string;
  useInOrchestrator: boolean;
  models: PinnedModel[];
  prefs: Prefs;
}

export interface CatalogModel {
  id: string;
  name: string;
  created: number;
  description: string;
  context: number;
  priceIn: number | null;
  priceOut: number | null;
  priceCacheRead: number | null;
  tools: boolean;
  reasoning: boolean;
  structured: boolean;
  imageIn: boolean;
}

export interface Catalog {
  fetched: number;
  models: CatalogModel[];
}

export interface Remote {
  usage: number | null;
  limit: number | null;
  remaining: number | null;
  freeTier: boolean | null;
  models: string[];
}

export const KIND_LABEL: Record<AccountKind, string> = { openrouter: 'OpenRouter', 'openai-compatible': 'OpenAI-compatible' };

export function emptyInput(kind: AccountKind): AccountInput {
  return { name: '', kind, baseUrl: '', useInOrchestrator: true, models: [], prefs: { ...DEFAULT_PREFS } };
}

export function inputOf(a: Account): AccountInput {
  return { name: a.name, kind: a.kind, baseUrl: a.baseUrl, useInOrchestrator: a.useInOrchestrator, models: a.models.map((m) => ({ ...m })), prefs: { ...a.prefs } };
}

/** Dollars per million tokens as a person reads them: $0.30, $15, free. */
export function price(p: number | null): string {
  if (p === null) return '?';
  if (p === 0) return 'free';
  if (p >= 100) return `$${Math.round(p)}`;
  return Number.isInteger(p) ? `$${p}` : `$${p.toFixed(2)}`;
}

/** "in / out" per million tokens, or a plain note when the price is not known. */
export function priceLine(m: { priceIn: number | null; priceOut: number | null }): string {
  if (m.priceIn === null && m.priceOut === null) return 'no price';
  return `${price(m.priceIn)} in · ${price(m.priceOut)} out`;
}

export function contextLabel(tokens: number): string {
  if (tokens <= 0) return '';
  if (tokens >= 1_000_000) return `${+(tokens / 1_000_000).toFixed(1)}M ctx`;
  return `${Math.round(tokens / 1000)}k ctx`;
}

/** How long ago a model was added, in the coarsest unit that reads well. */
export function ageLabel(created: number, now: number): string {
  if (created <= 0) return '';
  const days = Math.floor((now - created) / 86400);
  if (days < 1) return 'today';
  if (days < 14) return `${days}d ago`;
  if (days < 60) return `${Math.floor(days / 7)}w ago`;
  if (days < 730) return `${Math.floor(days / 30)}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

export type SortKey = 'newest' | 'cheapest' | 'context' | 'name';

export const SORTS: { value: SortKey; label: string }[] = [
  { value: 'newest', label: 'Newest' },
  { value: 'cheapest', label: 'Cheapest' },
  { value: 'context', label: 'Largest context' },
  { value: 'name', label: 'Name' },
];

/** The blended price used to rank by cost: input plus output, with unknown prices last. */
function cost(m: CatalogModel): number {
  return m.priceIn === null || m.priceOut === null ? Number.POSITIVE_INFINITY : m.priceIn + m.priceOut;
}

export function sortModels(models: CatalogModel[], by: SortKey): CatalogModel[] {
  const out = [...models];
  const byName = (a: CatalogModel, b: CatalogModel) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id);
  switch (by) {
    case 'newest':
      return out.sort((a, b) => b.created - a.created || byName(a, b));
    case 'cheapest':
      return out.sort((a, b) => cost(a) - cost(b) || byName(a, b));
    case 'context':
      return out.sort((a, b) => b.context - a.context || byName(a, b));
    case 'name':
      return out.sort(byName);
  }
}

export interface CatalogFilter {
  query: string;
  /** The id's vendor prefix ("anthropic"), or '' for all. */
  provider: string;
  /** Hide models that cannot call tools, which agents need. */
  toolsOnly: boolean;
}

export const providerOf = (id: string): string => (id.includes('/') ? id.slice(0, id.indexOf('/')) : '');

export function filterModels(models: CatalogModel[], f: CatalogFilter): CatalogModel[] {
  const words = f.query.toLowerCase().split(/\s+/).filter(Boolean);
  return models.filter((m) => {
    if (f.toolsOnly && !m.tools) return false;
    if (f.provider && providerOf(m.id) !== f.provider) return false;
    const hay = `${m.id} ${m.name}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** Vendors in the list, most models first. */
export function providersOf(models: CatalogModel[]): string[] {
  const n = new Map<string, number>();
  for (const m of models) {
    const p = providerOf(m.id);
    if (p) n.set(p, (n.get(p) ?? 0) + 1);
  }
  return [...n].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([p]) => p);
}

export function pinFromCatalog(m: CatalogModel): PinnedModel {
  return { id: m.id, priceIn: m.priceIn, priceOut: m.priceOut, tools: m.tools };
}

/** A model that an OpenAI-compatible endpoint listed: no price, and tools assumed. */
export function pinFromId(id: string): PinnedModel {
  return { id, priceIn: null, priceOut: null, tools: true };
}

/** Add the model if it is not pinned, remove it if it is. Returns a new list. */
export function togglePin(pinned: PinnedModel[], model: PinnedModel): PinnedModel[] {
  return pinned.some((p) => p.id === model.id) ? pinned.filter((p) => p.id !== model.id) : [...pinned, model];
}

/** A price typed into a field: blank is unknown, anything else must be a non-negative number. */
export function parsePrice(text: string): number | null | 'bad' {
  const t = text.trim().replace(/^\$/, '');
  if (t === '') return null;
  const n = Number(t);
  return Number.isFinite(n) && n >= 0 && n < 1e6 ? n : 'bad';
}

/** Remaining credit as a line for the account list, or null when OpenRouter did not say. */
export function balanceLine(r: Pick<Remote, 'usage' | 'limit' | 'remaining'>): string | null {
  const usd = (n: number) => `$${n.toFixed(2)}`;
  if (r.remaining !== null) return `${usd(r.remaining)} left`;
  if (r.usage !== null) return r.limit !== null ? `${usd(r.usage)} of ${usd(r.limit)} used` : `${usd(r.usage)} used`;
  return null;
}

/** The pinned models of accounts that may be used in flows, for the orchestrator's pickers. */
export function flowModels(accounts: Account[]): { account: Account; model: PinnedModel }[] {
  return accounts.filter((a) => a.useInOrchestrator && a.hasKey).flatMap((account) => account.models.map((model) => ({ account, model })));
}

// ---- sessions that run on OpenRouter ------------------------------------------------------

export type Driver = 'claude' | 'codex';

export const DRIVER_LABEL: Record<Driver, string> = { claude: 'Claude Code', codex: 'Codex' };

/** What the server runs a model with unless told otherwise: Anthropic's models in Claude Code, the rest in Codex. */
export function driverFor(model: string): Driver {
  const vendor = (model.split('/')[0] ?? '').replace(/^~/, '');
  return vendor === 'anthropic' ? 'claude' : 'codex';
}

/** A running session's lease, as `/api/openrouter/leases` lists it. */
export interface SessionLease {
  account: string;
  model: string;
  tag: string;
  capUsd: number | null;
  spent: { requests: number; costUsd: number };
}

/** Accounts a session can be started on. */
export const orAccounts = (accounts: Account[]): Account[] => accounts.filter((a) => a.kind === 'openrouter' && a.hasKey);

/** Dollars as a person reads them on a row: $0.18, or <$0.01 for a trace. */
export function usd(n: number): string {
  if (n <= 0) return '$0.00';
  return n < 0.01 ? '<$0.01' : `$${n.toFixed(2)}`;
}

/** The model without its vendor, for a crowded row: claude-haiku-5.5. */
export const shortModel = (id: string): string => id.slice(id.indexOf('/') + 1);

/** "$0.18 of $2.00", or just what has been spent when there is no cap. */
export function spendLine(spent: number, cap: number | null): string {
  return cap === null ? usd(spent) : `${usd(spent)} of ${usd(cap)}`;
}

/** The models of `account` a running session on `driver` can switch to, current one first. The others need a different CLI. */
export function switchable(account: Account, current: string, driver: Driver): { model: PinnedModel; ok: boolean; current: boolean }[] {
  const seen = new Set<string>();
  const rows: { model: PinnedModel; ok: boolean; current: boolean }[] = [];
  const add = (model: PinnedModel) => {
    if (seen.has(model.id)) return;
    seen.add(model.id);
    const isCurrent = model.id === current;
    rows.push({ model, ok: isCurrent || driverFor(model.id) === driver, current: isCurrent });
  };
  // The session's own model leads even if it was never pinned (it was typed, or the pin was removed since).
  add(account.models.find((m) => m.id === current) ?? { id: current, priceIn: null, priceOut: null, tools: true });
  for (const m of account.models) add(m);
  return rows;
}
