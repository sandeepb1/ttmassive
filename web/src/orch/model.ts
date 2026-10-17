// The logic of orchestrator mode that is not drawing: what a connection may be, how a flow is laid
// out, what a paused run is waiting for, and how full the plan windows are. Pure, so node tests it.
// The server checks everything again; these rules exist so the page can say no before asking.

import { DEFAULT_ITERATIONS, DEFAULT_LIMITS } from './types.ts';
import { orLines, orStrip, orUsedPct } from '../usage-format.ts';
import type {
  Advanced, Budget, Capability, CustomRole, EdgeKind, Flow, Turn, Routing, Rung, FlowEdge, FlowModel, OnCap, FlowNode, LogEntry, Models, Output, Pause, Question, Role, RoleDef, Run, RunStatus, RunSummary, Stage, Tokens, UsageSnapshot,
} from './types.ts';

// ---------------------------------------------------------------------------- roles

/** The six roles the first version shipped with: used until the server's list arrives, and in tests. */
const FIRST_SIX: RoleDef[] = [
  { id: 'orchestrator', label: 'Orchestrator', blurb: 'Plans the work', stage: 'plan', icon: 'workflow', capability: 'read', output: 'plan', core: true, checkpoint: false, prefer: 'claude', claudeModel: 'opus', prompt: 'You are the orchestrator. Turn the brief into a short, concrete plan the other agents can follow. Do not do the work yourself.' },
  { id: 'researcher', label: 'Researcher', blurb: 'Reads code and sources', stage: 'gather', icon: 'book', capability: 'read', output: 'report', core: true, checkpoint: false, prefer: 'claude', claudeModel: 'sonnet', prompt: 'You are a researcher. Find out what is needed to do the task well and report findings with their sources. Do not change files.' },
  { id: 'search', label: 'Search', blurb: 'Finds files and facts, fast', stage: 'gather', icon: 'search', capability: 'read', output: 'report', core: false, hidden: true, checkpoint: false, prefer: 'codex', claudeModel: 'haiku', codexModel: 'gpt-6-luna', prompt: 'You are a search agent. Locate the files, symbols and facts the task touches and report exact locations. Do not change files.' },
  { id: 'analyst', label: 'Analyst', blurb: 'Weighs findings, recommends', stage: 'gather', icon: 'scale', capability: 'read', output: 'report', core: false, hidden: true, checkpoint: false, prefer: 'claude', claudeModel: 'sonnet', prompt: 'You are an analyst. Weigh the findings you are given, point out risks, and recommend one approach with reasons. Do not change files.' },
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
  return { id, name, nodes: [], edges: [], maxRetries: 2, budget: { ...DEFAULT_LIMITS }, downshift: false, purpose: '' };
}

/** Longest a flow's purpose may be; the server refuses more. */
export const MAX_PURPOSE = 300;

/** The flow's purpose as one tidy line, or '' when it has none. */
export const purposeOf = (flow: Pick<Flow, 'purpose'>): string => (flow.purpose ?? '').replace(/\s+/g, ' ').trim();

/** What the run dialog says about the flow, so the goal typed there is about this time and not the kind of work. */
export function goalHint(flow: Pick<Flow, 'purpose'>): string {
  const purpose = purposeOf(flow);
  const ask = 'Say it the way you would to a colleague. The orchestrator will ask what it needs to know before it starts.';
  return purpose ? `This flow is for: ${purpose} ${ask}` : ask;
}

/** Whether the design assistant has anything to change: a flow with no agents is built with Design a flow first. */
export const canEdit = (flow: Pick<Flow, 'nodes'>): boolean => flow.nodes.length > 0;

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
      if (s.pause.kind === 'limit') return { text: 'Waiting', tone: 'warn' };
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
  /** Offer to let the agent that stopped send work back this many more times. */
  moreRounds?: number;
}

/** When a run parked on a usage limit will try again, as a person reads it: "6:01 pm", or with the day when it is not today. */
export function resumeLabel(resumeAt: number, now: number): string {
  const at = new Date(resumeAt * 1000);
  const time = at.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }).toLowerCase();
  const sameDay = at.toDateString() === new Date(now * 1000).toDateString();
  return sameDay ? time : `${at.toLocaleDateString([], { weekday: 'short', day: 'numeric', month: 'short' })}, ${time}`;
}

/** How long until then: "12 minutes", "3 hours 5 minutes", "2 days". */
export function untilLabel(resumeAt: number, now: number): string {
  const s = Math.max(0, resumeAt - now);
  if (s < 90) return 'a moment';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} minute${m === 1 ? '' : 's'}`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 === 0 ? `${h} hour${h === 1 ? '' : 's'}` : `${h} hour${h === 1 ? '' : 's'} ${m % 60} minutes`;
  const d = Math.round(h / 24);
  return `${d} day${d === 1 ? '' : 's'}`;
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
    case 'retriesExhausted': {
      const n = run.flow.nodes.find((x) => x.id === p.node);
      const tries = `${run.retriesUsed} ${run.retriesUsed === 1 ? 'round' : 'rounds'}`;
      // A Griller is not a judge: it has questions left, not a verdict, so the choice is about asking more or moving on.
      return n && asks(n)
        ? {
            title: `${name(p.node)} still has questions`,
            detail: `It has asked and been answered for ${tries} and wants to ask more. Carry on to the next agents with what it has, allow it more rounds, or stop here.`,
            primary: { label: 'Carry on without more questions', action: 'resume' },
            moreRounds: 2,
          }
        : {
            title: 'The work was sent back too many times',
            detail: `${name(p.node)} was still not satisfied after ${tries}. Read its feedback, then accept the work as it is, allow more rounds, or stop here.`,
            primary: { label: 'Accept the work as it is', action: 'resume' },
            moreRounds: 2,
          };
    }
    case 'limit': {
      const now = Math.floor(Date.now() / 1000);
      return {
        title: `${name(p.node)} hit a usage limit`,
        detail: `${p.message}. Nothing is wrong with the work: the run carries on by itself at ${resumeLabel(p.resumeAt, now)} (in ${untilLabel(p.resumeAt, now)}). You do not need to do anything.`,
        primary: { label: 'Try now', action: 'resume' },
      };
    }
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
  /** What kind of work the flow is for; empty for a flow with none or one that is not saved. */
  purpose: string;
  /** The folder it is kept in; empty at the top level. */
  folder: string;
  /** The directory its runs are tied to, because its folder or one above it is linked to one; empty when they are free. */
  dir: string;
  /** Newest first. */
  runs: RunSummary[];
  last: RunSummary | null;
}

/** The executions of one flow, newest first. */
export const runsOf = (runs: RunSummary[], flowId: string): RunSummary[] =>
  runs.filter((r) => r.flowId === flowId).sort((a, b) => b.created - a.created);

/** Every saved flow, and every flow that only survives as executions. The ones used most recently come first; a flow
 *  never run goes after them, by name. */
export function flowEntries(saved: Flow[], runs: RunSummary[], folderDirs: Record<string, string> = {}): FlowEntry[] {
  const entry = (id: string, name: string, isSaved: boolean, agents: number | null, purpose = '', folder = ''): FlowEntry => {
    const mine = runsOf(runs, id);
    return { id, name, saved: isSaved, agents, purpose, folder, dir: linkedDir(folderDirs, folder)?.dir ?? '', runs: mine, last: mine[0] ?? null };
  };
  const out = saved.map((f) => entry(f.id, f.name, true, f.nodes.length, purposeOf(f), f.folder ?? ''));
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

/** Web-service credits the run's agents have used. */
export function runCredits(run: Run): number {
  return Object.values(run.nodes).reduce((n, x) => n + (x.credits ?? 0), 0);
}

/** Dollars the run's web access has cost: the agents' tools, the pages and docs fetched for them, and the research before it. */
export function runWebUsd(run: Run): number {
  const agents = Object.values(run.nodes).reduce((n, x) => n + (x.webUsd ?? 0), 0);
  const pages = [...(run.references ?? []), ...(run.docs ?? [])].reduce((n, x) => n + (x.webUsd ?? 0), 0);
  return agents + pages + (run.researchWebUsd ?? 0);
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
  if (t.credits > 0) out.push(`${t.credits} web credit${t.credits === 1 ? '' : 's'}${(t.webUsd ?? 0) > 0 ? ` (${usdLabel(t.webUsd ?? 0)})` : ''}`);
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

/** What it takes to start a run like an earlier one: the same agents and settings as it ran with, the same brief and
 *  project. Pages and documentation it had are fetched again only if it had some. */
export function rerunRequest(r: Run): { flow: Flow; goal: string; project: string; skipGrill: true; brief?: string; readLinks: boolean; docs: string[] } {
  return {
    flow: r.flow,
    goal: r.goal,
    project: r.project,
    skipGrill: true,
    brief: r.brief || undefined,
    readLinks: (r.references?.length ?? 0) > 0,
    docs: (r.docs ?? []).map((d) => d.url),
  };
}

/** The roles to offer for a new agent: the ones that are not kept only for flows that already use them. */
export const offered = (roles: RoleDef[]): RoleDef[] => roles.filter((r) => !r.hidden);

// ---- folders and the table of flows ------------------------------------------------------------

export const parentOf = (path: string): string => path.split('/').slice(0, -1).join('/');
export const baseOf = (path: string): string => path.split('/').pop() ?? '';
export const childPath = (parent: string, name: string): string => (parent ? `${parent}/${name}` : name);
/** Whether `path` is `folder` or inside it. */
export const inFolder = (path: string, folder: string): boolean => path === folder || path.startsWith(`${folder}/`);

/** A folder name as it will be kept: no slashes, trimmed, short. Empty when nothing usable is left. */
export const cleanFolderName = (text: string): string => text.replace(/[\/\u0000-\u001f]/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 40);

/** The path to a folder as clickable steps, starting at the top level. */
export function crumbs(at: string): { name: string; path: string }[] {
  const out = [{ name: 'Flows', path: '' }];
  let path = '';
  for (const name of at.split('/').filter(Boolean)) {
    path = childPath(path, name);
    out.push({ name, path });
  }
  return out;
}

export type SortKey = 'name' | 'agents' | 'runs' | 'last';
export interface Sort {
  key: SortKey;
  /** 1 for ascending, -1 for descending. */
  dir: 1 | -1;
}
/** Flows run most recently first. */
export const DEFAULT_SORT: Sort = { key: 'last', dir: -1 };

/** The sort after a column heading is chosen: the same column again turns it round, another starts at its natural end. */
export function sortBy(now: Sort, key: SortKey): Sort {
  if (now.key === key) return { key, dir: now.dir === 1 ? -1 : 1 };
  return { key, dir: key === 'name' ? 1 : -1 };
}

export function sortEntries(list: FlowEntry[], sort: Sort): FlowEntry[] {
  const value = (e: FlowEntry): number | string => (sort.key === 'name' ? e.name.toLowerCase() : sort.key === 'agents' ? (e.agents ?? -1) : sort.key === 'runs' ? e.runs.length : (e.last?.created ?? -1));
  return [...list].sort((a, b) => {
    const [x, y] = [value(a), value(b)];
    const by = typeof x === 'string' ? x.localeCompare(y as string) : (x as number) - (y as number);
    return by * sort.dir || a.name.localeCompare(b.name);
  });
}

/** Where a folder, or the nearest one above it, is linked to a directory: that folder and its directory. `null` when none is. */
export function linkedDir(dirs: Record<string, string>, folder: string): { folder: string; dir: string } | null {
  let at = folder;
  for (;;) {
    const dir = dirs[at];
    if (dir) return { folder: at, dir };
    if (!at) return null;
    at = parentOf(at);
  }
}

/** A directory with your home folder shortened to `~`, as it is easiest to read. */
export const tildePath = (dir: string): string => dir.replace(/^\/(?:Users|home)\/[^/]+(?=\/|$)/, '~');

export interface FolderRow {
  path: string;
  name: string;
  /** The directory this folder is itself linked to; empty when it is not. */
  dir: string;
  /** Flows in it and in the folders inside it. */
  flows: number;
  /** Folders directly inside it. */
  folders: number;
  /** Executions of the flows in it and in the folders inside it, and the latest of them. */
  runs: number;
  last: RunSummary | null;
}

export interface Listing {
  crumbs: { name: string; path: string }[];
  folders: FolderRow[];
  flows: FlowEntry[];
  /** The folder being looked at, or the one above it, is linked to a directory: its flows run there. */
  linked: { folder: string; dir: string } | null;
  /** A search is on: the flows are from every folder and the folders are left out. */
  searching: boolean;
}

/** Every folder, including those only implied by where a flow is kept, parents first. */
export function allFolders(folders: string[], entries: Pick<FlowEntry, 'folder'>[]): string[] {
  const set = new Set<string>();
  for (const path of [...folders, ...entries.map((e) => e.folder)]) {
    let at = '';
    for (const name of path.split('/').filter(Boolean)) {
      at = childPath(at, name);
      set.add(at);
    }
  }
  return [...set].sort((a, b) => a.localeCompare(b));
}

/** What the flows table shows for the folder `at`: the folders directly inside it, then its flows. A search looks in every folder. */
export function listing(entries: FlowEntry[], folders: string[], at: string, query: string, sort: Sort, dirs: Record<string, string> = {}): Listing {
  const q = query.trim().toLowerCase();
  if (q) {
    const hit = (e: FlowEntry) => [e.name, e.purpose, e.folder].some((s) => s.toLowerCase().includes(q));
    return { crumbs: crumbs(at), folders: [], flows: sortEntries(entries.filter(hit), sort), linked: linkedDir(dirs, at), searching: true };
  }
  const every = allFolders(folders, entries);
  const rows = every
    .filter((p) => parentOf(p) === at && p !== at)
    .map((p): FolderRow => {
      const within = entries.filter((e) => inFolder(e.folder, p));
      const ran = within.flatMap((e) => e.runs);
      return { path: p, name: baseOf(p), dir: dirs[p] ?? '', flows: within.length, folders: every.filter((q2) => parentOf(q2) === p).length, runs: ran.length, last: ran.reduce<RunSummary | null>((a, r) => (!a || r.created > a.created ? r : a), null) };
    })
    .sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
  return { crumbs: crumbs(at), folders: rows, flows: sortEntries(entries.filter((e) => e.folder === at), sort), linked: linkedDir(dirs, at), searching: false };
}

/** Where a flow can be moved to: the top level, then every folder. */
export const moveTargets = (folders: string[]): { path: string; label: string }[] => [{ path: '', label: 'Flows (top level)' }, ...folders.map((p) => ({ path: p, label: p.replace(/\//g, ' › ') }))];

// ---- the table of executions -------------------------------------------------------------------

export type RunGroup = 'all' | 'active' | 'waiting' | 'done' | 'other';

export const RUN_GROUPS: { id: RunGroup; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'active', label: 'Running' },
  { id: 'waiting', label: 'Needs you' },
  { id: 'done', label: 'Done' },
  { id: 'other', label: 'Stopped' },
];

/** Which heading an execution is under: going now, waiting for a person or a limit, finished, or stopped. */
export function runGroup(s: RunStatus): Exclude<RunGroup, 'all'> {
  switch (s.state) {
    case 'running':
    case 'grilling':
      return 'active';
    case 'paused':
      return 'waiting';
    case 'done':
      return 'done';
    default:
      return 'other';
  }
}

export function runCounts(runs: RunSummary[]): Record<RunGroup, number> {
  const counts: Record<RunGroup, number> = { all: runs.length, active: 0, waiting: 0, done: 0, other: 0 };
  for (const r of runs) counts[runGroup(r.status)] += 1;
  return counts;
}

/** The executions under a heading that match the words typed (in the goal or the flow's name), newest first. */
export function filterRuns(runs: RunSummary[], group: RunGroup, query: string): RunSummary[] {
  const q = query.trim().toLowerCase();
  return runs.filter((r) => (group === 'all' || runGroup(r.status) === group) && (!q || r.goal.toLowerCase().includes(q) || r.flowName.toLowerCase().includes(q) || (r.project ?? '').toLowerCase().includes(q))).sort((a, b) => b.created - a.created);
}

// ---- advanced settings of an agent --------------------------------------------------------------

/** The bounds the server holds each setting to. */
export const ADV = { timeMax: 24 * 3600, calls: 1000, retries: 3, charsMin: 500, charsMax: 200_000, tools: 20 } as const;

export const isDefaultAdvanced = (a: Advanced | undefined): boolean =>
  !a || (a.timeLimitSecs == null && a.costCapUsd == null && a.maxToolCalls == null && !a.autoRetries && !a.fallback && a.inputs == null && a.inputChars == null && !a.freshOnRetry && !a.shell && !(a.denyTools?.length));

/** How many of an agent's advanced settings are set, for the heading that says so. */
export function advancedCount(a: Advanced | undefined): number {
  if (!a) return 0;
  return [a.timeLimitSecs != null, a.costCapUsd != null, a.maxToolCalls != null, !!a.autoRetries, !!a.fallback, a.inputs != null, a.inputChars != null, !!a.freshOnRetry, !!a.shell, !!a.denyTools?.length].filter(Boolean).length;
}

/** A time typed into a field: `90s`, `5m`, `2h`, or several together such as `1h 30m`; a lone bare number is minutes. Blank is no limit. */
export function parseDuration(text: string): number | null | 'bad' {
  const t = text.trim().toLowerCase();
  if (t === '') return null;
  const part = /(\d+(?:\.\d+)?)\s*(seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h)?\s*/y;
  let secs = 0;
  let at = 0;
  let parts = 0;
  while (at < t.length) {
    part.lastIndex = at;
    const m = part.exec(t);
    // Every part needs its unit, unless it is the only one.
    if (!m || (!m[2] && parts > 0)) return 'bad';
    const unit = m[2]?.[0];
    secs += Number(m[1]) * (unit === 's' ? 1 : unit === 'h' ? 3600 : 60);
    at = part.lastIndex;
    parts += 1;
  }
  secs = Math.round(secs);
  return secs >= 1 && secs <= ADV.timeMax ? secs : 'bad';
}

/** The other way, exactly: `90s`, `5m`, `1h 30m`, `1h 1m 1s`. */
export function formatDuration(secs: number): string {
  const [h, m, s] = [Math.floor(secs / 3600), Math.floor((secs % 3600) / 60), secs % 60];
  return [h && `${h}h`, m && `${m}m`, s && `${s}s`].filter(Boolean).join(' ') || '0s';
}

/** A whole number in a range typed into a field; blank is none. */
export function parseWhole(text: string, min: number, max: number): number | null | 'bad' {
  const t = text.trim();
  if (t === '') return null;
  const n = Number(t);
  return Number.isInteger(n) && n >= min && n <= max ? n : 'bad';
}

/** Dollars typed into a field; blank is no cap. */
export function parseUsdCap(text: string): number | null | 'bad' {
  const t = text.trim().replace(/^\$/, '');
  if (t === '') return null;
  const n = Number(t);
  return Number.isFinite(n) && n > 0 && n < 1e6 ? n : 'bad';
}

/** Tool names typed in, separated by commas or lines: trimmed, each once. `bad` when a name is not one, or there are too many. */
export function parseToolNames(text: string): string[] | 'bad' {
  const names = [...new Set(text.split(/[\n,]/).map((t) => t.trim()).filter(Boolean))];
  const ok = names.length <= ADV.tools && names.every((n) => n.length <= 80 && /^[A-Za-z0-9_:*().\- ]+$/.test(n));
  return ok ? names : 'bad';
}

/** The agents whose results `node` can be shown: the ones that feed it, in the order they are on the chart. */
export function feeders(flow: Pick<Flow, 'nodes' | 'edges'>, nodeId: string): FlowNode[] {
  const ids = new Set(flow.edges.filter((e) => e.kind === 'forward' && e.to === nodeId).map((e) => e.from));
  return flow.nodes.filter((n) => ids.has(n.id));
}

/** The settings with a change made, kept tidy: a fallback that is the same as nothing, and an empty list of inputs, go away. */
export function withAdvanced(a: Advanced | undefined, patch: Partial<Advanced>): Advanced {
  const next: Advanced = { ...(a ?? {}), ...patch };
  if (next.autoRetries != null) next.autoRetries = Math.max(0, Math.min(ADV.retries, Math.round(next.autoRetries)));
  if (!next.autoRetries) next.fallback = null;
  if (next.inputs && next.inputs.length === 0) next.inputs = null;
  return next;
}

/** After a connection or an agent is removed, take out of every agent's `inputs` any that no longer feed it, as the server does. */
export function pruneInputs(flow: Flow): Flow {
  let changed = false;
  const nodes = flow.nodes.map((n) => {
    const list = n.advanced?.inputs;
    if (!list) return n;
    const fed = new Set(feeders(flow, n.id).map((f) => f.id));
    const kept = list.filter((i) => fed.has(i));
    if (kept.length === list.length) return n;
    changed = true;
    return { ...n, advanced: { ...n.advanced, inputs: kept.length ? kept : null } };
  });
  return changed ? { ...flow, nodes } : flow;
}

/** The models an agent's retry may be sent to, as picker entries: the same pool a ladder draws on. */
export function fallbackChoices(models: Models): { key: string; label: string; rung: Rung }[] {
  const several = new Set(models.openrouter.map((m) => m.account)).size > 1;
  const plan = (engine: 'claude' | 'codex', list: string[]) => list.map((model) => ({ key: rungKey({ engine, model, account: null }), label: `${engine === 'claude' ? 'Claude' : 'Codex'} · ${model}`, rung: { engine, model, account: null, effort: null } as Rung }));
  const paid = models.openrouter
    .filter((m) => !m.direct)
    .map((m) => ({ key: `or:${m.account}:${m.id}`, label: several ? `${m.accountName} · ${m.id}` : m.id, rung: { engine: m.driver, model: m.id, account: m.account, effort: null } as Rung }));
  return [...plan('claude', models.claude), ...plan('codex', models.codex), ...paid];
}

// ---- designing a flow, in three steps ---------------------------------------------------------

export type DesignStep = 1 | 2 | 3;

/** What the page says while a step of designing a flow is under way. */
export const DESIGN_STEP_TEXT: Record<DesignStep, string> = {
  1: 'Step 1 of 3 · Looking at your plans, accounts and models, and designing a team for the work',
  2: 'Step 2 of 3 · Checking it has the right agents, on models that fit and that you have room for',
  3: 'Step 3 of 3 · Writing each agent’s prompt for the model it runs on',
};

export interface DesignNote {
  step: 'Design' | 'Review' | 'Prompts';
  text: string;
  /** The step could not run, and the design before it stands. */
  failed?: boolean;
}

/** What came of each step, for the notes shown once the flow is made. A step that failed says so and leaves the earlier result. */
export function designReport(design: string[], review: { ok: true; summary: string } | { ok: false; error: string }, prompts: { ok: true; summary: string } | { ok: false; error: string }): DesignNote[] {
  const notes: DesignNote[] = design.map((text) => ({ step: 'Design' as const, text }));
  notes.push(review.ok ? { step: 'Review', text: review.summary } : { step: 'Review', text: `It could not be done (${review.error}), so the first design stands.`, failed: true });
  notes.push(prompts.ok ? { step: 'Prompts', text: prompts.summary } : { step: 'Prompts', text: `It could not be done (${prompts.error}), so the prompts are as the design first wrote them.`, failed: true });
  return notes;
}
