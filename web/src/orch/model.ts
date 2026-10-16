// The logic of orchestrator mode that is not drawing: what a connection may be, how a flow is laid
// out, what a paused run is waiting for, and how full the plan windows are. Pure, so node tests it.
// The server checks everything again; these rules exist so the page can say no before asking.

import { DEFAULT_ITERATIONS, DEFAULT_LIMITS } from './types.ts';
import { orLines, orStrip, orUsedPct } from '../usage-format.ts';
import type {
  Budget, Capability, CustomRole, EdgeKind, Flow, Turn, Routing, Rung, FlowEdge, FlowModel, OnCap, FlowNode, LogEntry, Models, Output, Pause, Question, Role, RoleDef, Run, RunStatus, RunSummary, Stage, Tokens, UsageSnapshot,
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
/** Hands back questions, so it may send work back until they are answered. */
export const asks = (n: Pick<FlowNode, 'role' | 'custom'>) => outputOf(n) === 'questions';
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
  questions: 'Open questions',
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
    web: !!def.web,
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
  if (!decides(a) && !asks(a)) return 'Only an agent that gives a verdict or asks questions can send work back';
  if (asks(a) && flow.edges.some((e) => e.from === from && e.kind === 'retry')) return 'An agent that asks questions can send work back to only one place';
  if (!reaches(flow.edges, to, from)) return 'Send work back to an agent that runs before this one';
  if (decides(b) || asks(b)) return 'Send work back to an agent that does not itself send work back';
  return null;
}

export function addEdge(flow: Flow, from: string, to: string, kind: EdgeKind): Flow {
  const id = freeId(flow.edges.map((e) => e.id), 'e');
  // A send-back edge starts with the usual limits: twice, then stop and ask; an agent that asks questions asks the user.
  const retry: Partial<FlowEdge> = kind === 'retry' ? { maxIterations: DEFAULT_ITERATIONS, onCap: 'pause', answeredBy: 'user' } : {};
  return { ...flow, edges: [...flow.edges, { id, from, to, kind, ...retry }] };
}

/** The text on a send-back edge: what it is, and how often it may be used, or has been. */
export function edgeLabel(e: FlowEdge, run: Run | null): string | undefined {
  if (e.kind !== 'retry') return undefined;
  const max = e.maxIterations ?? DEFAULT_ITERATIONS;
  return run ? `send back ${run.iterations?.[e.id] ?? 0}/${max}` : `send back ×${max}`;
}

/** What "used up" does, in words for the editor. */
export const ON_CAP_TEXT: Record<OnCap, string> = {
  pause: 'Stop and ask me',
  proceed: 'Carry on with the work as it is',
  fail: 'Fail the agent that wanted to send it back',
};

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
  primary: { label: string; action: 'approve' | 'resume' | 'answer'; node?: string; /** Also ignore the plan limits from now on. */ waive?: boolean } | null;
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
    case 'questions':
      return {
        title: `${name(p.node)} has questions for you`,
        detail: 'Your answers go back to the agent that is asked to revise its work. Nothing runs until you answer.',
        primary: { label: 'Answer the questions', action: 'answer' },
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

/** A question without a number in front of it: the dialog numbers them itself, and a model sometimes writes one too. */
export function unnumbered(q: string): string {
  return q.replace(/^\s*\d{1,2}[.):]\s+/, '').trim();
}

/** The questions a run is waiting on, as the grill shows them: with choices when the agent gave some, else plain text. */
export function waitingQuestions(p: { questions: string[]; details?: Question[] }): Question[] {
  if (p.details && p.details.length === p.questions.length) return p.details.map((q) => ({ ...q, text: unnumbered(q.text) }));
  return p.questions.map((text) => ({ text: unnumbered(text), recommended: '', options: [], multiple: false }));
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

// ---------------------------------------------------------------------------- OpenRouter models and what they cost

/** The picker's value for an agent's model: `plan:<model>` for one on your plan, `or:<account>:<model>` for OpenRouter. */
export function modelKey(node: Pick<FlowNode, 'account' | 'model'>): string {
  return node.account ? `or:${node.account}:${node.model}` : `plan:${node.model}`;
}

/** The other way. An OpenRouter model id may hold colons (`…:free`), an account id never does. */
export function parseModelKey(key: string): { account: string | null; model: string } {
  if (key.startsWith('or:')) {
    const rest = key.slice(3);
    const at = rest.indexOf(':');
    if (at > 0) return { account: rest.slice(0, at), model: rest.slice(at + 1) };
  }
  return { account: null, model: key.replace(/^plan:/, '') };
}

/**
 * What choosing `key` in the model picker changes. An OpenRouter model brings its account, and the CLI that runs
 * it unless one is already running it. A model on your plan clears the account.
 */
export function modelPatch(key: string, models: Models): Partial<FlowNode> {
  const { account, model } = parseModelKey(key);
  if (!account) return { model, account: null, direct: false };
  const known = models.openrouter.find((m) => m.account === account && m.id === model);
  return known ? { model, account, direct: known.direct, ...(known.direct ? {} : { engine: known.driver }) } : { model, account };
}

/** Where an agent can run: your plan's CLI, or an account (OpenRouter, or a direct call on an OpenAI-compatible one). */
export interface Provider {
  key: string;
  label: string;
  direct: boolean;
}

/** The places an agent can run, with the one it is on first if its account is no longer on offer. */
export function providersFor(node: Pick<FlowNode, 'account' | 'direct'>, models: Models): Provider[] {
  const out: Provider[] = [{ key: 'plan', label: 'My Claude / Codex plan', direct: false }];
  const seen = new Set<string>();
  for (const m of models.openrouter) {
    if (seen.has(m.account)) continue;
    seen.add(m.account);
    out.push({ key: `or:${m.account}`, label: `${m.direct ? 'API' : 'OpenRouter'} · ${m.accountName}${m.direct ? ' (direct call, no tools)' : ''}`, direct: m.direct });
  }
  if (node.account && !seen.has(node.account)) out.push({ key: `or:${node.account}`, label: 'An account that is no longer available', direct: !!node.direct });
  return out;
}

/** The provider key an agent is on. */
export const providerOf = (node: Pick<FlowNode, 'account'>): string => (node.account ? `or:${node.account}` : 'plan');

/** Moving an agent to another place: its model becomes the first one there, and it keeps its engine where the CLI is its own. */
export function providerPatch(key: string, node: Pick<FlowNode, 'engine'>, models: Models): Partial<FlowNode> {
  if (key === 'plan') {
    const list = node.engine === 'claude' ? models.claude : models.codex;
    return { account: null, direct: false, model: list[0] ?? '' };
  }
  const account = key.slice(3);
  const first = models.openrouter.find((m) => m.account === account);
  return first ? modelPatch(`or:${account}:${first.id}`, models) : { account };
}

/** The pinned OpenRouter models, with the node's own first if it is no longer on offer (its account was changed or removed). */
export function openRouterChoices(node: FlowNode, models: Models): { key: string; label: string; offered: boolean; direct: boolean }[] {
  // The account's name only helps when there is more than one to tell apart.
  const several = new Set(models.openrouter.map((m) => m.account)).size > 1;
  const out = models.openrouter.map((m) => ({ key: `or:${m.account}:${m.id}`, label: several ? `${m.accountName} · ${m.id}` : m.id, offered: true, direct: m.direct }));
  const mine = node.account ? modelKey(node) : null;
  if (mine && !out.some((o) => o.key === mine)) out.unshift({ key: mine, label: `${node.model} (account unavailable)`, offered: false, direct: !!node.direct });
  return out;
}

/** Dollars OpenRouter charged for the run: only the agents on it, since the others were not billed. */
export function runCost(run: Run): number {
  return run.flow.nodes.filter((n) => n.account).reduce((sum, n) => sum + (run.nodes[n.id]?.costUsd ?? 0), 0);
}

/** One OpenRouter account's meter in the top bar: what the key has left (or has spent), and this run's part of it. */
export interface OrGauge {
  id: string;
  label: string;
  /** How much of the key's limit is used, 0 to 100; null for a key with no limit. */
  used: number | null;
  /** The figure shown: what is left, or what was spent today. */
  text: string;
  /** What this run has spent on API models in all, shown beside it. */
  runUsd: number | null;
  tone: Tone;
  title: string;
}

/** Every OpenRouter account with a key, as a meter; the run's spend is shown on the accounts its agents use. */
export function orGauges(flow: Flow, run: Run | null, usage: UsageSnapshot | null, b: Budget): OrGauge[] {
  const used = new Set(flow.nodes.map((n) => n.account).filter((a): a is string => !!a));
  const keys = usage?.openrouter ?? [];
  const spent = run ? runCost(run) : 0;
  return keys.map((u) => {
    const pct = orUsedPct(u);
    const strip = orStrip(u);
    let tone: Tone = strip.level === 'crit' ? 'bad' : strip.level === 'warn' ? 'warn' : 'ok';
    if (b.runUsd && spent >= b.runUsd) tone = 'bad';
    else if (b.runUsd && spent >= b.runUsd * 0.8 && tone === 'ok') tone = 'warn';
    const lines = u.error ? [u.error] : orLines(u);
    if (b.runUsd) lines.push(`this run may spend ${usdLabel(b.runUsd)}`);
    return { id: u.id, label: `OpenRouter${keys.length > 1 ? ` · ${u.name}` : ''}`, used: pct, text: strip.text, runUsd: run && used.has(u.id) ? spent : null, tone, title: lines.join(' · ') };
  });
}

/** Dollars as a person reads them. A flow's agents often cost fractions of a cent, so small amounts keep their digits. */
export function usdLabel(n: number): string {
  if (n <= 0) return '$0.00';
  if (n < 0.0001) return '<$0.0001';
  if (n < 0.01) return `$${n.toFixed(4)}`;
  return `$${n.toFixed(n < 10 ? 3 : 2).replace(/(\.\d\d)0$/, '$1')}`;
}

/** What the model costs per million tokens, as a line under the picker. */
export function priceNote(m: Pick<FlowModel, 'priceIn' | 'priceOut'>): string {
  const p = (v: number | null) => (v === null ? '?' : v === 0 ? 'free' : `$${Number.isInteger(v) ? v : v.toFixed(2)}`);
  return m.priceIn === null && m.priceOut === null ? 'No price on file' : `${p(m.priceIn)} in · ${p(m.priceOut)} out, per million tokens`;
}

/** Whether an agent in this role could call its model directly: it must need no tools. */
export const canCallDirectly = (n: Pick<FlowNode, 'role' | 'custom'>): boolean => capabilityOf(n) === 'read';

/** Firecrawl credits the run's agents have used. */
export function runCredits(run: Run): number {
  return Object.values(run.nodes).reduce((n, x) => n + (x.credits ?? 0), 0);
}

// ---------------------------------------------------------------------------- the timeline of a run

export interface Round {
  /** How many send-backs had happened when these turns started: 0 is the first pass. */
  round: number;
  turns: Turn[];
  /** Why the next round began, when it did: the decision in the turn that sent work back. */
  because: string | null;
}

/** Turns grouped into rounds, in the order they finished. A round starts when work was sent back. */
export function rounds(turns: Turn[]): Round[] {
  const out: Round[] = [];
  for (const t of turns) {
    let r = out.at(-1);
    if (!r || r.round !== t.round) {
      // The turn that ended the round before this one is the one that sent the work back.
      const prev = out.at(-1)?.turns.at(-1);
      r = { round: t.round, turns: [], because: null };
      if (prev) out.at(-1)!.because = prev.decision ?? null;
      out.push(r);
    }
    r.turns.push(t);
  }
  return out;
}

/** "3s", "1m 04s": how long a turn took. */
export function turnLength(t: Pick<Turn, 'started' | 'ended'>): string {
  return fmtDuration(Math.max(0, Math.round((t.ended - t.started) / 1000)));
}

/** The wall-clock time of a turn's start, like 14:03:22. */
export function turnClock(ms: number): string {
  const d = new Date(ms);
  const two = (n: number) => String(n).padStart(2, '0');
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

export type TurnTone = 'ok' | 'bad' | 'warn' | 'plain';

/** How a turn went, for its marker: failed, sent work back or asked, or just fine. */
export function turnTone(t: Turn): TurnTone {
  if (t.failed) return 'bad';
  if (t.decision && /sent (it|the work) back|asked \d+ question|has questions/i.test(t.decision)) return 'warn';
  return 'ok';
}

/** The one line that says what a turn was on and what it cost. */
export function turnFacts(t: Turn, nodeName: string): string[] {
  const out = [t.engine === 'direct' ? 'direct call' : t.engine === 'claude' ? 'Claude Code' : 'Codex', t.model];
  if (t.effort) out.push(`${t.effort} effort`);
  out.push(turnLength(t), `${fmtTokens(totalTokens(t.tokens))} tok`);
  if (t.account && t.costUsd > 0) out.push(usdLabel(t.costUsd));
  if (t.credits > 0) out.push(`${t.credits} web credit${t.credits === 1 ? '' : 's'}`);
  if (t.attempt > 1) out.push(`${nodeName}, try ${t.attempt}`);
  return out;
}

/** Turns of one agent only, or all of them. */
export const turnsOf = (turns: Turn[], node: string | null): Turn[] => (node ? turns.filter((t) => t.node === node) : turns);

// ---------------------------------------------------------------------------- choosing models per turn

export const ROUTING_TEXT: Record<Routing, string> = {
  pinned: 'Fixed: always this model',
  ladder: 'Ladder: rules move it up and down',
  auto: 'Auto: a small model picks each turn',
};

export const DIAL_TEXT = ['Strongly prefer low cost', 'Lean toward low cost', 'Balance cost and quality', 'Lean toward quality', 'Strongly prefer quality'] as const;

/** A rung's picker value: where the model comes from, and which it is. */
export function rungKey(r: Pick<Rung, 'engine' | 'model' | 'account'>): string {
  return r.account ? `or:${r.account}:${r.model}` : `${r.engine}:${r.model}`;
}

/** The rung a picker value stands for. The effort a rung had is kept when the model changes. */
export function rungFromKey(key: string, models: Models, effort?: string | null): Rung {
  if (key.startsWith('or:')) {
    const { account, model } = parseModelKey(key);
    const known = models.openrouter.find((m) => m.account === account && m.id === model);
    return { engine: known?.driver ?? 'claude', model, account, effort: effort ?? null };
  }
  const at = key.indexOf(':');
  return { engine: key.slice(0, at) === 'codex' ? 'codex' : 'claude', model: key.slice(at + 1), account: null, effort: effort ?? null };
}

/** A new rung to add below the last: the agent's own model, or the next one down the list when that is already there. */
export function nextRung(node: Pick<FlowNode, 'engine' | 'model' | 'account' | 'ladder'>, models: Models): Rung {
  const have = new Set((node.ladder ?? []).map(rungKey));
  const own: Rung = { engine: node.engine, model: node.model, account: node.account ?? null, effort: null };
  if (!have.has(rungKey(own))) return own;
  const pool: Rung[] = [
    ...models.claude.map((m) => ({ engine: 'claude' as const, model: m, account: null, effort: null })),
    ...models.codex.map((m) => ({ engine: 'codex' as const, model: m, account: null, effort: null })),
    ...models.openrouter.filter((m) => !m.direct).map((m) => ({ engine: m.driver, model: m.id, account: m.account, effort: null })),
  ];
  return pool.find((r) => !have.has(rungKey(r))) ?? own;
}

/** The node with its ladder changed; a node that goes back to fixed keeps the ladder, in case it comes back. */
export const withRoutingChange = (node: FlowNode, routing: Routing, models: Models): Partial<FlowNode> =>
  routing !== 'pinned' && !(node.ladder?.length) ? { routing, ladder: [nextRung(node, models)] } : { routing };

/** The one line under a turn's model that says what the dial and ladder did, for an agent on a ladder. */
export const ladderSummary = (n: Pick<FlowNode, 'routing' | 'ladder'>): string | null =>
  n.routing && n.routing !== 'pinned' && n.ladder?.length ? `${n.ladder.length} rung${n.ladder.length === 1 ? '' : 's'}, ${n.routing === 'auto' ? 'router picks' : 'by rules'}` : null;

/** Whether a goal contains a web address, which a web account can read for the agents. */
export const hasLink = (text: string): boolean => /https?:\/\/[^\s<>"'`]{4,}/.test(text);
