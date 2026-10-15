// The logic of orchestrator mode that is not drawing: what a connection may be, how a flow is laid
// out, what a paused run is waiting for, and how full the plan windows are. Pure, so node tests it.
// The server checks everything again; these rules exist so the page can say no before asking.

import type {
  Budget, EdgeKind, Flow, FlowEdge, FlowNode, Models, Pause, Role, Run, RunStatus, Tokens, UsageSnapshot,
} from './types.ts';

export const ROLES: Role[] = ['orchestrator', 'researcher', 'search', 'analyst', 'judge', 'builder'];

export const ROLE_INFO: Record<Role, { label: string; blurb: string }> = {
  orchestrator: { label: 'Orchestrator', blurb: 'Plans the work' },
  researcher: { label: 'Researcher', blurb: 'Reads code and sources' },
  search: { label: 'Search', blurb: 'Finds files and facts, fast' },
  analyst: { label: 'Analyst', blurb: 'Weighs findings, recommends' },
  judge: { label: 'Judge', blurb: 'Reviews and can send work back' },
  builder: { label: 'Builder', blurb: 'The only one that changes files' },
};

const MAX_NODES = 24;
const LAYER_W = 280;
const ROW_H = 190;

// ---------------------------------------------------------------------------- flows

/** The first id `prefix1`, `prefix2`... not already taken. */
export function freeId(taken: string[], prefix: string): string {
  const used = new Set(taken);
  for (let i = 1; ; i++) if (!used.has(`${prefix}${i}`)) return `${prefix}${i}`;
}

/** Cheap and quick for reading, the strongest model for the roles that decide. Every node can change it. */
export function defaultModel(role: Role, models: Models): { engine: 'claude' | 'codex'; model: string } {
  const codex = (name: string) => models.codex.includes(name);
  if (role === 'orchestrator' || role === 'judge') return { engine: 'claude', model: 'opus' };
  if (role === 'builder') return codex('gpt-6.1-sol') ? { engine: 'codex', model: 'gpt-6.1-sol' } : { engine: 'claude', model: 'sonnet' };
  if (role === 'search') return codex('gpt-6-luna') ? { engine: 'codex', model: 'gpt-6-luna' } : { engine: 'claude', model: 'haiku' };
  return { engine: 'claude', model: 'sonnet' };
}

const DEFAULT_PROMPT: Record<Role, string> = {
  orchestrator: 'You are the orchestrator. Turn the brief into a short, concrete plan the other agents can follow. Do not do the work yourself.',
  researcher: 'You are a researcher. Find out what is needed to do the task well and report findings with their sources. Do not change files.',
  search: 'You are a search agent. Locate the files, symbols and facts the task touches and report exact locations. Do not change files.',
  analyst: 'You are an analyst. Weigh the findings you are given, point out risks, and recommend one approach with reasons. Do not change files.',
  judge: 'You are a strict reviewer. Approve only if the work is correct and complete; otherwise reject and say exactly what to fix.',
  builder: 'You are the builder. Do the work described by the plan and findings, keep changes small, run the checks, and report what you changed.',
};

export function newNode(flow: Flow, role: Role, models: Models, at: { x: number; y: number }): FlowNode {
  const { engine, model } = defaultModel(role, models);
  const same = flow.nodes.filter((n) => n.role === role).length;
  const label = ROLE_INFO[role].label;
  return {
    id: freeId(flow.nodes.map((n) => n.id), 'n'),
    name: same ? `${label} ${same + 1}` : label,
    role,
    engine,
    model,
    systemPrompt: DEFAULT_PROMPT[role],
    // A builder changes files, so it waits for the user by default.
    checkpoint: role === 'builder',
    tokenCap: null,
    keepModel: false,
    x: Math.round(at.x),
    y: Math.round(at.y),
  };
}

export function emptyFlow(id: string, name: string): Flow {
  return { id, name, nodes: [], edges: [], maxRetries: 2, budget: { claudePct: 15, codexPct: 10, stopWindowPct: 85 }, downshift: false };
}

export function removeNode(flow: Flow, id: string): Flow {
  return { ...flow, nodes: flow.nodes.filter((n) => n.id !== id), edges: flow.edges.filter((e) => e.from !== id && e.to !== id) };
}

/** Can `to` be reached from `from` along forward edges? */
function reaches(edges: FlowEdge[], from: string, to: string): boolean {
  const seen = new Set<string>();
  const stack = [from];
  while (stack.length) {
    const n = stack.pop()!;
    if (n === to) return true;
    if (seen.has(n)) continue;
    seen.add(n);
    for (const e of edges) if (e.kind === 'forward' && e.from === n) stack.push(e.to);
  }
  return false;
}

/** Why this connection is not allowed, or null if it is. */
export function connectionProblem(flow: Flow, from: string, to: string, kind: EdgeKind): string | null {
  const a = flow.nodes.find((n) => n.id === from);
  const b = flow.nodes.find((n) => n.id === to);
  if (!a || !b) return 'Both ends must be agents in the flow';
  if (from === to) return 'An agent cannot hand work to itself';
  if (flow.edges.some((e) => e.from === from && e.to === to && e.kind === kind)) return 'Already connected';
  if (kind === 'forward') {
    if (reaches(flow.edges, to, from)) return 'That would make a loop; only a judge’s retry edge may go back';
    return null;
  }
  if (a.role !== 'judge') return 'Only a judge can send work back';
  if (b.role !== 'builder') return 'A judge can only send work back to a builder';
  if (!reaches(flow.edges, to, from)) return 'Send work back to an agent that runs before the judge';
  return null;
}

export function addEdge(flow: Flow, from: string, to: string, kind: EdgeKind): Flow {
  const id = freeId(flow.edges.map((e) => e.id), 'e');
  return { ...flow, edges: [...flow.edges, { id, from, to, kind }] };
}

/** Agents grouped by when they can start (a judge's retry edge is not a dependency). Empty if there is a loop. */
export function layers(flow: Flow): string[][] {
  const indeg = new Map(flow.nodes.map((n) => [n.id, 0]));
  for (const e of flow.edges) if (e.kind === 'forward') indeg.set(e.to, (indeg.get(e.to) ?? 0) + 1);
  const out: string[][] = [];
  let placed = 0;
  for (;;) {
    const ready = flow.nodes.map((n) => n.id).filter((id) => indeg.get(id) === 0);
    if (!ready.length) break;
    for (const id of ready) indeg.delete(id);
    for (const e of flow.edges) if (e.kind === 'forward' && ready.includes(e.from) && indeg.has(e.to)) indeg.set(e.to, indeg.get(e.to)! - 1);
    placed += ready.length;
    out.push(ready);
  }
  return placed === flow.nodes.length ? out : [];
}

/** Left to right by when each agent can start, top to bottom within a column. */
export function tidy(flow: Flow): Flow {
  const cols = layers(flow);
  if (!cols.length) return flow;
  const pos = new Map<string, { x: number; y: number }>();
  cols.forEach((col, c) => col.forEach((id, r) => pos.set(id, { x: c * LAYER_W, y: r * ROW_H })));
  return { ...flow, nodes: flow.nodes.map((n) => ({ ...n, ...(pos.get(n.id) ?? {}) })) };
}

/** Run order, for the list view on a phone. */
export function runOrder(flow: Flow): FlowNode[] {
  const byId = new Map(flow.nodes.map((n) => [n.id, n]));
  const ordered = layers(flow).flat().map((id) => byId.get(id)!);
  return ordered.length ? ordered : flow.nodes;
}

/** What is wrong with a flow, or null if it can be saved and run. Mirrors the server's checks. */
export function flowProblem(flow: Flow): string | null {
  if (!flow.name.trim()) return 'Give the flow a name';
  if (!flow.nodes.length) return 'Add at least one agent';
  if (flow.nodes.length > MAX_NODES) return `A flow can have at most ${MAX_NODES} agents`;
  const ids = new Set(flow.nodes.map((n) => n.id));
  if (ids.size !== flow.nodes.length) return 'Two agents share an id';
  if (flow.nodes.some((n) => !n.name.trim())) return 'Every agent needs a name';
  if (flow.nodes.some((n) => !n.model.trim())) return 'Every agent needs a model';
  if (flow.edges.some((e) => !ids.has(e.from) || !ids.has(e.to))) return 'An edge points at an agent that is gone';
  if (!layers(flow).length) return 'The flow has a loop; only a judge’s retry edge may go back';
  const rest = { ...flow, edges: [] as FlowEdge[] };
  for (const e of flow.edges.filter((x) => x.kind === 'retry')) {
    const why = connectionProblem({ ...rest, edges: flow.edges.filter((x) => x.id !== e.id) }, e.from, e.to, 'retry');
    if (why) return why;
  }
  const b = flow.budget;
  for (const v of [b.claudePct, b.codexPct, b.stopWindowPct]) if (!(v > 0 && v <= 100)) return 'Budget percentages must be between 1 and 100';
  if (flow.maxRetries < 0 || flow.maxRetries > 5) return 'Retries must be 0 to 5';
  return null;
}

// ---------------------------------------------------------------------------- runs

export type Tone = 'idle' | 'live' | 'warn' | 'ok' | 'bad';

export function statusLabel(s: RunStatus): { text: string; tone: Tone } {
  switch (s.state) {
    case 'grilling': return { text: 'Questions', tone: 'warn' };
    case 'running': return { text: 'Running', tone: 'live' };
    case 'done': return { text: 'Done', tone: 'ok' };
    case 'cancelled': return { text: 'Cancelled', tone: 'idle' };
    case 'paused':
      return s.pause.kind === 'failed' || s.pause.kind === 'retriesExhausted'
        ? { text: 'Needs you', tone: 'bad' }
        : { text: 'Paused', tone: 'warn' };
  }
}

export interface PauseInfo {
  title: string;
  detail: string;
  /** What the main button does; none when the user must change something first. */
  primary: { label: string; action: 'approve' | 'resume'; node?: string } | null;
}

export function pauseInfo(run: Run): PauseInfo | null {
  if (run.status.state !== 'paused') return null;
  const p: Pause = run.status.pause;
  const name = (id: string) => run.flow.nodes.find((n) => n.id === id)?.name ?? id;
  switch (p.kind) {
    case 'approval': {
      const n = run.flow.nodes.find((x) => x.id === p.node);
      const writes = n && n.role === 'builder';
      return {
        title: `${name(p.node)} is ready to start`,
        detail: writes ? 'It will change files in its own copy of the project. Look at the plan, then approve.' : 'Look at what came before, then approve.',
        primary: { label: `Approve ${name(p.node)}`, action: 'approve', node: p.node },
      };
    }
    case 'retriesExhausted':
      return {
        title: 'The judge sent the work back too many times',
        detail: `${name(p.node)} was still not satisfied after ${run.retriesUsed} ${run.retriesUsed === 1 ? 'retry' : 'retries'}. Read its feedback, then accept the work as it is, or stop here.`,
        primary: { label: 'Accept the work as it is', action: 'resume' },
      };
    case 'budget':
      return { title: 'Stopped to protect your plan limits', detail: p.message, primary: { label: 'Carry on anyway', action: 'resume' } };
    case 'failed':
      return {
        title: p.node ? `${name(p.node)} stopped` : 'The run was interrupted',
        detail: p.message,
        primary: { label: 'Try again', action: 'resume' },
      };
  }
}

/** Where a builder's work is kept: its own branch in the project, in its own worktree. */
export function builderBranches(run: Run): { node: string; name: string; branch: string }[] {
  return run.flow.nodes
    .filter((n) => n.role === 'builder' && (run.nodes[n.id]?.attempts ?? 0) > 0)
    .map((n) => ({ node: n.id, name: n.name, branch: `ttm-orch/${run.id}-${n.id}` }));
}

/** Agents the run cannot start until you approve them, even while others are still working: every agent that
 *  asks first, has all its inputs done, and has not been let through. */
export function awaitingApproval(run: Run): string[] {
  return run.flow.nodes
    .filter((n) => n.checkpoint && !run.approved.includes(n.id))
    .filter((n) => run.nodes[n.id]?.status === 'pending')
    .filter((n) => run.flow.edges.filter((e) => e.kind === 'forward' && e.to === n.id).every((e) => run.nodes[e.from]?.status === 'done'))
    .map((n) => n.id);
}

/** The model an agent ran on, and whether that is cheaper than the one the flow asked for. */
export function modelShown(node: FlowNode, nr?: { modelUsed?: string | null }): { label: string; cheaper: boolean } {
  const used = nr?.modelUsed;
  return used && used !== node.model ? { label: `${node.model} → ${used}`, cheaper: true } : { label: used ?? node.model, cheaper: false };
}

export const cacheBreakCount = (run: Run): number => Object.values(run.nodes).reduce((n, x) => n + (x.cacheBreaks ?? 0), 0);

export function fmtDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`;
}

export interface Summary {
  duration: number | null;
  tokens: number;
  cached: number | null;
  sentBack: number;
  cheaper: number;
  cacheBreaks: number;
}

/** What to say about a finished run. */
export function summary(run: Run): Summary {
  const t = runTokens(run);
  const cheaper = run.flow.nodes.filter((n) => modelShown(n, run.nodes[n.id]).cheaper).length;
  return {
    duration: run.finished ? run.finished - run.created : null,
    tokens: totalTokens(t),
    cached: cacheHit(t),
    sentBack: run.retriesUsed,
    cheaper,
    cacheBreaks: cacheBreakCount(run),
  };
}

export const sumTokens = (list: Tokens[]): Tokens =>
  list.reduce((a, t) => ({ input: a.input + t.input, output: a.output + t.output, cacheRead: a.cacheRead + t.cacheRead, cacheWrite: a.cacheWrite + t.cacheWrite }), { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });

export const runTokens = (run: Run): Tokens => sumTokens([run.overhead, ...Object.values(run.nodes).map((n) => n.tokens)]);

/** Everything the model read or wrote, cache included. */
export const totalTokens = (t: Tokens) => t.input + t.output + t.cacheRead + t.cacheWrite;

/** Share of input served from the cache, 0 to 1; null before any input. */
export function cacheHit(t: Tokens): number | null {
  const input = t.input + t.cacheRead + t.cacheWrite;
  return input > 0 ? t.cacheRead / input : null;
}

export function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`;
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

// ---------------------------------------------------------------------------- usage

export interface Gauge {
  engine: 'claude' | 'codex';
  label: string;
  /** How full the 5-hour window is, 0 to 100. Null if it could not be read. */
  used: number | null;
  /** How far it has moved since the run began; this is the run's share (plus anything else using the plan). */
  share: number | null;
  budget: number;
  stop: number;
  tone: Tone;
}

export function gauges(flow: Flow, run: Run | null, usage: UsageSnapshot | null): Gauge[] {
  const b: Budget = flow.budget;
  const engines = (['claude', 'codex'] as const).filter((e) => flow.nodes.some((n) => n.engine === e));
  return engines.map((engine) => {
    const w = usage?.[engine]?.five_hour ?? null;
    const used = w ? w.used_pct : null;
    const base = run ? run.baseline[engine] : null;
    const share = used !== null && base !== null && base !== undefined ? Math.max(0, used - base) : null;
    const budget = engine === 'claude' ? b.claudePct : b.codexPct;
    let tone: Tone = 'ok';
    if ((used !== null && used >= b.stopWindowPct) || (share !== null && share >= budget)) tone = 'bad';
    else if ((used !== null && used >= b.stopWindowPct - 10) || (share !== null && share >= budget * 0.8)) tone = 'warn';
    return { engine, label: engine === 'claude' ? 'Claude 5h' : 'Codex 5h', used, share, budget, stop: b.stopWindowPct, tone };
  });
}

// ---------------------------------------------------------------------------- grilling

/** Answers to send: what was typed, or the recommended answer where nothing was. */
export function answersFor(pending: { recommended: string }[], typed: Record<number, string>): string[] {
  return pending.map((q, i) => (typed[i] ?? '').trim() || q.recommended);
}
