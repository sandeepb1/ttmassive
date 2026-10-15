// The logic of orchestrator mode that is not drawing: what a connection may be, how a flow is laid
// out, what a paused run is waiting for, and how full the plan windows are. Pure, so node tests it.
// The server checks everything again; these rules exist so the page can say no before asking.

import { DEFAULT_LIMITS } from './types.ts';
import type {
  Budget, Capability, CustomRole, EdgeKind, Flow, FlowEdge, FlowNode, LogEntry, Models, Output, Pause, Question, Role, RoleDef, Run, RunStatus, RunSummary, Stage, Tokens, UsageSnapshot,
} from './types.ts';

// ---------------------------------------------------------------------------- roles

/** The six roles the first version shipped with: used until the server's list arrives, and in tests. */
const FIRST_SIX: RoleDef[] = [
  { id: 'orchestrator', label: 'Orchestrator', blurb: 'Plans the work', stage: 'plan', icon: 'workflow', capability: 'read', output: 'plan', core: true, checkpoint: false, prefer: 'claude', claudeModel: 'opus', prompt: 'You are the orchestrator. Turn the brief into a short, concrete plan the other agents can follow. Do not do the work yourself.' },
  { id: 'researcher', label: 'Researcher', blurb: 'Reads code and sources', stage: 'gather', icon: 'book', capability: 'read', output: 'report', core: true, checkpoint: false, prefer: 'claude', claudeModel: 'sonnet', prompt: 'You are a researcher. Find out what is needed to do the task well and report findings with their sources. Do not change files.' },
  { id: 'search', label: 'Search', blurb: 'Finds files and facts, fast', stage: 'gather', icon: 'search', capability: 'read', output: 'report', core: true, checkpoint: false, prefer: 'codex', claudeModel: 'haiku', codexModel: 'gpt-6-luna', prompt: 'You are a search agent. Locate the files, symbols and facts the task touches and report exact locations. Do not change files.' },
  { id: 'analyst', label: 'Analyst', blurb: 'Weighs findings, recommends', stage: 'gather', icon: 'scale', capability: 'read', output: 'report', core: true, checkpoint: false, prefer: 'claude', claudeModel: 'sonnet', prompt: 'You are an analyst. Weigh the findings you are given, point out risks, and recommend one approach with reasons. Do not change files.' },
  { id: 'builder', label: 'Builder', blurb: 'The one that changes files', stage: 'build', icon: 'hammer', capability: 'write', output: 'change', core: true, checkpoint: true, prefer: 'codex', claudeModel: 'sonnet', codexModel: 'gpt-6.1-sol', prompt: 'You are the builder. Do the work described by the plan and findings, keep changes small, run the checks, and report what you changed.' },
  { id: 'judge', label: 'Judge', blurb: 'Reviews, and can send work back', stage: 'check', icon: 'gavel', capability: 'inspect', output: 'verdict', core: true, checkpoint: false, prefer: 'claude', claudeModel: 'opus', prompt: 'You are a strict reviewer. Approve only if the work is correct and complete; otherwise reject and say exactly what to fix.' },
];

let catalog: RoleDef[] = FIRST_SIX;

/** The roles the server offers. Called once they have loaded. */
export function setCatalog(defs: RoleDef[]) {
  catalog = defs.length ? defs : FIRST_SIX;
}
export const getCatalog = (): RoleDef[] => catalog;
export const roleDef = (id: Role): RoleDef | undefined => catalog.find((d) => d.id === id);

/** What an agent may do. A custom role says so itself; a built-in one is looked up. */
export const capabilityOf = (n: Pick<FlowNode, 'role' | 'custom'>): Capability => n.custom?.capability ?? roleDef(n.role)?.capability ?? 'read';
export const outputOf = (n: Pick<FlowNode, 'role' | 'custom'>): Output => n.custom?.output ?? roleDef(n.role)?.output ?? 'report';
/** Changes files, in its own worktree. */
export const writes = (n: Pick<FlowNode, 'role' | 'custom'>) => capabilityOf(n) === 'write';
/** Hands back a decision, so it may send work back to an earlier agent. */
export const decides = (n: Pick<FlowNode, 'role' | 'custom'>) => outputOf(n) === 'verdict';
export const roleLabel = (n: Pick<FlowNode, 'role' | 'custom'>): string => n.custom?.label ?? roleDef(n.role)?.label ?? 'Agent';
export const roleBlurb = (n: Pick<FlowNode, 'role' | 'custom'>): string => (n.custom ? 'A role you defined' : (roleDef(n.role)?.blurb ?? ''));
export const roleIconName = (n: Pick<FlowNode, 'role' | 'custom'>): string => (n.custom ? 'wand' : (roleDef(n.role)?.icon ?? 'agent'));

export const CAPABILITY_TEXT: Record<Capability, string> = {
  read: 'Reads and searches',
  inspect: 'Reads, and runs checks in a sandbox',
  write: 'Changes files in its own copy',
};
/** Short enough for a dropdown. */
export const CAPABILITY_SHORT: Record<Capability, string> = {
  read: 'Read only',
  inspect: 'Read and run checks',
  write: 'Change files',
};

export const OUTPUT_TEXT: Record<Output, string> = {
  plan: 'A plan',
  report: 'A report of findings',
  change: 'A change it made',
  verdict: 'A verdict: approve or reject',
};

/** The same rules the server applies to a role it generates: a writer reports a change and nothing else does,
 *  so a reviewer can never come out able to edit what it reviews. */
export function consistent(capability: Capability, output: Output): Output {
  if (capability === 'write') return 'change';
  return output === 'change' ? 'report' : output;
}

export const STAGES: { id: Stage; label: string }[] = [
  { id: 'plan', label: 'Plan' },
  { id: 'gather', label: 'Gather' },
  { id: 'build', label: 'Build' },
  { id: 'check', label: 'Check' },
  { id: 'deliver', label: 'Deliver' },
];

const MAX_NODES = 24;
const LAYER_W = 280;
const ROW_H = 190;

// ---------------------------------------------------------------------------- flows

/** The first id `prefix1`, `prefix2`... not already taken. */
export function freeId(taken: string[], prefix: string): string {
  const used = new Set(taken);
  for (let i = 1; ; i++) if (!used.has(`${prefix}${i}`)) return `${prefix}${i}`;
}

/** The model a new agent of this role starts on: what the role prefers, if that model is on offer. */
export function defaultModel(def: RoleDef, models: Models): { engine: 'claude' | 'codex'; model: string } {
  if (def.prefer === 'codex' && def.codexModel && models.codex.includes(def.codexModel)) return { engine: 'codex', model: def.codexModel };
  return { engine: 'claude', model: models.claude.includes(def.claudeModel) ? def.claudeModel : (models.claude[0] ?? def.claudeModel) };
}

const uniqueName = (flow: Flow, label: string) => {
  const same = flow.nodes.filter((n) => roleLabel(n) === label).length;
  return same ? `${label} ${same + 1}` : label;
};

export function newNode(flow: Flow, def: RoleDef, models: Models, at: { x: number; y: number }): FlowNode {
  const { engine, model } = defaultModel(def, models);
  return {
    id: freeId(flow.nodes.map((n) => n.id), 'n'),
    name: uniqueName(flow, def.label),
    role: def.id,
    engine,
    model,
    systemPrompt: def.prompt,
    checkpoint: def.checkpoint,
    tokenCap: null,
    keepModel: false,
    x: Math.round(at.x),
    y: Math.round(at.y),
  };
}

/** An agent in a role the user designed. It starts on a balanced Claude model; every node can change it. */
export function newCustomNode(flow: Flow, role: CustomRole, instructions: string, models: Models, at: { x: number; y: number }): FlowNode {
  return {
    id: freeId(flow.nodes.map((n) => n.id), 'n'),
    name: uniqueName(flow, role.label),
    role: 'custom',
    custom: { ...role, output: consistent(role.capability, role.output) },
    engine: 'claude',
    model: models.claude.includes('sonnet') ? 'sonnet' : (models.claude[0] ?? 'sonnet'),
    systemPrompt: instructions,
    checkpoint: role.capability === 'write',
    tokenCap: null,
    keepModel: false,
    x: Math.round(at.x),
    y: Math.round(at.y),
  };
}

/** A copy of an agent beside the original, with no connections: the user wires it where it belongs. */
export function duplicateNode(flow: Flow, id: string): { flow: Flow; node: FlowNode } | null {
  const src = flow.nodes.find((n) => n.id === id);
  if (!src) return null;
  const node: FlowNode = { ...structuredClone(src), id: freeId(flow.nodes.map((n) => n.id), 'n'), name: uniqueName(flow, roleLabel(src)), x: src.x + 40, y: src.y + 40 };
  return { flow: { ...flow, nodes: [...flow.nodes, node] }, node };
}

/** What the log panel shows: everything, only skills and sub-agents, or one agent's lines. */
export type LogFilter = 'all' | 'skills' | `node:${string}`;

export function logView(log: LogEntry[], filter: LogFilter): LogEntry[] {
  if (filter === 'all') return log;
  if (filter === 'skills') return log.filter((e) => e.kind === 'skill' || e.kind === 'subagent');
  const id = filter.slice(5);
  return log.filter((e) => e.node === id);
}

/** How many skills were used and sub-agents started, for the filter's badge. */
export function skillCount(log: LogEntry[]): number {
  return log.filter((e) => e.kind === 'skill' || e.kind === 'subagent').length;
}

/** The time of day a log line happened, "14:03:09". */
export function clock(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
}

/** What each agent that is working right now last said it was doing, for the end of the log. */
export function liveLines(run: Run): { node: string; line: string }[] {
  const out: { node: string; line: string }[] = [];
  for (const n of run.flow.nodes) {
    const s = run.nodes[n.id];
    if (s?.status !== 'running') continue;
    out.push({ node: n.id, line: s.transcript[s.transcript.length - 1] ?? 'starting…' });
  }
  return out;
}

/** An execution that has stopped for good, so it can be removed without stopping anything. */
export const isFinished = (s: { state: string }) => s.state === 'done' || s.state === 'cancelled';

export function emptyFlow(id: string, name: string): Flow {
  return { id, name, nodes: [], edges: [], maxRetries: 2, budget: { ...DEFAULT_LIMITS }, downshift: false };
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
    if (reaches(flow.edges, to, from)) return 'That would make a loop; only an agent that gives a verdict may send work back';
    return null;
  }
  if (!decides(a)) return 'Only an agent that gives a verdict can send work back';
  if (!reaches(flow.edges, to, from)) return 'Send work back to an agent that runs before this one';
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
  if (!layers(flow).length) return 'The flow has a loop; only an agent that gives a verdict may send work back';
  const rest = { ...flow, edges: [] as FlowEdge[] };
  for (const e of flow.edges.filter((x) => x.kind === 'retry')) {
    const why = connectionProblem({ ...rest, edges: flow.edges.filter((x) => x.id !== e.id) }, e.from, e.to, 'retry');
    if (why) return why;
  }
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
  primary: { label: string; action: 'approve' | 'resume'; node?: string; /** Also ignore the plan limits from now on. */ waive?: boolean } | null;
  /** Offer to change the plan limits: the pause is because of them. */
  limits?: boolean;
}

export function pauseInfo(run: Run): PauseInfo | null {
  if (run.status.state !== 'paused') return null;
  const p: Pause = run.status.pause;
  const name = (id: string) => run.flow.nodes.find((n) => n.id === id)?.name ?? id;
  switch (p.kind) {
    case 'approval': {
      const n = run.flow.nodes.find((x) => x.id === p.node);
      const changes = !!n && writes(n);
      return {
        title: `${name(p.node)} is ready to start`,
        detail: changes ? 'It will change files in its own copy of the project. Look at the plan, then approve.' : 'Look at what came before, then approve.',
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
      return {
        title: 'Stopped to protect your plan limits',
        detail: p.message,
        primary: { label: 'Carry on anyway', action: 'resume', waive: true },
        limits: true,
      };
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
    .filter((n) => writes(n) && (run.nodes[n.id]?.attempts ?? 0) > 0)
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

// ---------------------------------------------------------------------------- flows and their executions

/** A flow as the sidebar lists it, with its executions. */
export interface FlowEntry {
  id: string;
  name: string;
  /** In the saved list. False for a flow that was never saved or has been deleted but still has executions. */
  saved: boolean;
  agents: number | null;
  /** Newest first. */
  runs: RunSummary[];
  last: RunSummary | null;
}

/** The executions of one flow, newest first. */
export const runsOf = (runs: RunSummary[], flowId: string): RunSummary[] =>
  runs.filter((r) => r.flowId === flowId).sort((a, b) => b.created - a.created);

/** Every saved flow, and every flow that only survives as executions. The ones used most recently come first; a flow
 *  never run goes after them, by name. */
export function flowEntries(saved: Flow[], runs: RunSummary[]): FlowEntry[] {
  const entry = (id: string, name: string, isSaved: boolean, agents: number | null): FlowEntry => {
    const mine = runsOf(runs, id);
    return { id, name, saved: isSaved, agents, runs: mine, last: mine[0] ?? null };
  };
  const out = saved.map((f) => entry(f.id, f.name, true, f.nodes.length));
  const known = new Set(saved.map((f) => f.id));
  for (const id of new Set(runs.map((r) => r.flowId))) {
    if (known.has(id)) continue;
    const latest = runsOf(runs, id)[0]!;
    out.push(entry(id, latest.flowName, false, null));
  }
  return out.sort((a, b) => (b.last?.created ?? -1) - (a.last?.created ?? -1) || a.name.localeCompare(b.name));
}

export function runsLabel(n: number): string {
  return n === 0 ? 'Never run' : n === 1 ? '1 run' : `${n} runs`;
}

/** "Today 19:04", "Yesterday 08:10", "12 Oct 14:20": when an execution started, for a list that keeps history. */
export function whenLabel(unixSeconds: number, now: Date = new Date()): string {
  const d = new Date(unixSeconds * 1000);
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(now) - day(d)) / 86_400_000);
  if (days === 0) return `Today ${hm}`;
  if (days === 1) return `Yesterday ${hm}`;
  const month = d.toLocaleString('en-GB', { month: 'short' });
  return `${d.getDate()} ${month}${d.getFullYear() === now.getFullYear() ? '' : ` ${d.getFullYear()}`} ${hm}`;
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

export function gauges(flow: Flow, run: Run | null, usage: UsageSnapshot | null, b: Budget): Gauge[] {
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

/** What the user has done with one question. */
export interface Answer {
  /** The options ticked. */
  chosen: string[];
  /** "Something else" was chosen, so `text` is the answer. */
  other: boolean;
  text: string;
}

/** A question as first shown: the recommended answer ticked, so continuing takes the suggestion. */
export function startingAnswer(q: Question): Answer {
  return { chosen: q.options?.length && q.recommended ? [q.recommended] : [], other: false, text: '' };
}

/** Does this question offer choices? Without any it is a text box with the suggestion as its placeholder. */
export const hasChoices = (q: Question): boolean => (q.options?.length ?? 0) >= 2;

/** Tick or untick an option. A single-choice question holds one; a multiple-choice one holds any number, in the order
 *  the options are offered. Choosing an option puts "Something else" down, and the other way round. */
export function choose(q: Question, pick: Answer, option: string): Answer {
  const options = q.options ?? [];
  if (!q.multiple) return { ...pick, chosen: [option], other: false };
  const has = pick.chosen.includes(option);
  const next = options.filter((o) => (o === option ? !has : pick.chosen.includes(o)));
  return { ...pick, chosen: next, other: false };
}

/** Switch to writing an answer. In a multiple-choice question the ticks stay, and the words are added to them. */
export function writeInstead(q: Question, pick: Answer): Answer {
  return { ...pick, other: true, chosen: q.multiple ? pick.chosen : [] };
}

/** The words to send for one question. */
export function answerFor(q: Question, pick: Answer): string {
  const typed = pick.text.trim();
  if (!hasChoices(q)) return typed || q.recommended;
  const picked = pick.chosen.filter((o) => q.options!.includes(o));
  if (pick.other) return q.multiple ? [...picked, typed].filter(Boolean).join('; ') || q.recommended : typed || q.recommended;
  return picked.length ? picked.join('; ') : q.recommended;
}

/** The answers to send, in order. Anything left untouched takes the suggestion. */
export function answersFor(pending: Question[], picks: Record<number, Answer>): string[] {
  return pending.map((q, i) => answerFor(q, picks[i] ?? startingAnswer(q)));
}
