import type { OrUsage } from '../usage-format.ts';
// The server's orchestrator JSON, as the page sees it. Field names are the ones src/orch/*.rs write.

/** A built-in role's id, or `custom`. The server's catalog (`/api/orch/roles`) says which exist. */
export type Role = string;
export type Capability = 'read' | 'inspect' | 'write';
export type Output = 'plan' | 'report' | 'change' | 'verdict' | 'questions';
export type Stage = 'plan' | 'gather' | 'build' | 'check' | 'deliver';

/** What makes a custom role what it is; kept on the node. */
export interface CustomRole {
  label: string;
  capability: Capability;
  output: Output;
}

/** A built-in role, as the server describes it. */
export interface RoleDef {
  id: Role;
  label: string;
  blurb: string;
  stage: Stage;
  icon: string;
  capability: Capability;
  output: Output;
  core: boolean;
  /** Kept so flows that use it still run, but not offered for a new agent. */
  hidden?: boolean;
  checkpoint: boolean;
  prefer: Engine;
  claudeModel: string;
  codexModel?: string | null;
  /** Given the web tools by default when there is a web account. */
  web?: boolean;
  prompt: string;
}
export type Engine = 'claude' | 'codex';
export type EdgeKind = 'forward' | 'retry';

export interface FlowNode {
  id: string;
  name: string;
  role: Role;
  /** Present exactly when `role` is `custom`. */
  custom?: CustomRole | null;
  engine: Engine;
  model: string;
  /** The OpenRouter account this agent runs on, when `model` is one of its models. */
  account?: string | null;
  /** Called directly, with no CLI and so no tools. */
  direct?: boolean;
  /** How hard it thinks, when set. */
  effort?: string | null;
  /** How its model is chosen each time it starts. */
  routing?: Routing;
  /** The models and efforts it may run on, cheapest and lightest first. */
  ladder?: Rung[];
  /** Given web search and page fetching (Firecrawl), when there is an account for it. */
  web?: boolean;
  systemPrompt: string;
  checkpoint: boolean;
  tokenCap?: number | null;
  /** Never switch this agent to a cheaper model. */
  keepModel?: boolean;
  x: number;
  y: number;
}

export type Routing = 'pinned' | 'ladder' | 'auto';

/** One step of an agent's ladder: a model, where it runs, and how hard it thinks. */
export interface Rung {
  engine: Engine;
  model: string;
  /** The OpenRouter account it runs on, when it does. */
  account?: string | null;
  effort?: string | null;
}

export const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'] as const;
export const DEFAULT_DIAL = 2;

export type OnCap = 'pause' | 'proceed' | 'fail';
export type Answerer = 'user' | 'agent';

export interface FlowEdge {
  id: string;
  from: string;
  to: string;
  kind: EdgeKind;
  /** A send-back edge: how many times work may go back along it in one run. */
  maxIterations?: number;
  /** What happens when it has been used that many times. */
  onCap?: OnCap;
  /** From an agent that asks questions: who answers them. */
  answeredBy?: Answerer;
}

export const DEFAULT_ITERATIONS = 2;

export interface Budget {
  claudePct: number;
  codexPct: number;
  stopWindowPct: number;
  /** Most dollars one run may spend on OpenRouter; null for no cap. */
  runUsd?: number | null;
  /** Most Firecrawl credits one run may use; null for no cap. */
  runCredits?: number | null;
}

export const DEFAULT_LIMITS: Budget = { claudePct: 15, codexPct: 10, stopWindowPct: 85, runUsd: null, runCredits: null };

export interface Flow {
  id: string;
  name: string;
  nodes: FlowNode[];
  edges: FlowEdge[];
  maxRetries: number;
  budget: Budget;
  /** Give agents that have not started yet a cheaper model as the plan windows fill up. */
  downshift?: boolean;
  /** Where agents with a ladder start: 0 the cheapest rung, 4 the strongest. */
  dial?: number;
}

export interface Tokens {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export type NodeStatus = 'pending' | 'running' | 'done' | 'failed' | 'interrupted';

export interface NodeRun {
  status: NodeStatus;
  attempts: number;
  output: unknown;
  error: string | null;
  tokens: Tokens;
  /** Dollars over all attempts: charged by OpenRouter, or Claude Code's estimate at API prices for an agent on your plan. */
  costUsd?: number;
  /** Firecrawl credits its web tools used. */
  credits?: number;
  transcript: string[];
  /** The agent's conversation id after its latest finished turn; present once it has finished one. */
  session?: string | null;
  /** The model it actually ran on: the flow's, or a cheaper one. */
  modelUsed?: string | null;
  /** How hard it was set to think on its latest turn. */
  effortUsed?: string | null;
  /** Why its latest turn ran on the model and effort it did. */
  routeReason?: string | null;
  /** Times a turn got far less from the prompt cache than the one before it. */
  cacheBreaks?: number;
}

export type Pause =
  | { kind: 'approval'; node: string }
  | { kind: 'retriesExhausted'; node: string }
  | { kind: 'budget'; message: string }
  | { kind: 'failed'; node: string; message: string }
  | { kind: 'questions'; node: string }
  /** An agent hit its plan's usage limit; the run carries on by itself at `resumeAt` (unix seconds). */
  | { kind: 'limit'; node: string; message: string; resumeAt: number };

export type RunStatus =
  | { state: 'grilling' }
  | { state: 'running' }
  | { state: 'done' }
  | { state: 'cancelled' }
  | { state: 'paused'; pause: Pause };

export interface Question {
  text: string;
  /** The answer the orchestrator would pick; one of `options` when there are any. */
  recommended: string;
  /** Two to four answers to choose from. Absent or empty for a question that is answered in words. */
  options?: string[];
  /** More than one option can be chosen together. */
  multiple?: boolean;
}

export interface Grill {
  round: number;
  history: { question: string; answer: string }[];
  pending: Question[];
  brief: string | null;
}

export type LogKind = 'decision' | 'request' | 'response' | 'skill' | 'subagent' | 'event' | 'error';

/** One line of a run's log: what was sent, what came back, what was decided and why. */
export interface LogEntry {
  at: number;
  node: string | null;
  kind: LogKind;
  title: string;
  body?: string | null;
}

export type RepoState = 'ready' | 'noCommits' | 'notARepo' | 'missing';

export interface Run {
  id: string;
  flow: Flow;
  goal: string;
  brief: string;
  project: string;
  grill: Grill | null;
  baseline: { claude: number | null; codex: number | null };
  overhead: Tokens;
  status: RunStatus;
  nodes: Record<string, NodeRun>;
  approved: string[];
  feedback: Record<string, string>;
  retriesUsed: number;
  /** Every turn the agents have taken, oldest first. */
  turns?: Turn[];
  /** Pages fetched from the links in the goal. */
  references?: { url: string; text: string; credits: number; failed?: boolean }[];
  /** Documentation sites crawled for the builders. */
  docs?: { url: string; text: string; credits: number; failed?: boolean }[];
  /** Dollars spent outside the agents, such as on choosing their models. */
  overheadUsd?: number;
  /** How many entries the run's log has ever had; it holds only the latest. */
  logTotal?: number;
  /** How many times each send-back edge has been used, by edge id. */
  iterations?: Record<string, number>;
  /** What agents that ask questions have asked, with the answers, by node id. */
  asked?: Record<string, { question: string; answer: string | null }[]>;
  /** Questions the run is waiting for the user to answer. */
  pending?: { node: string; target: string; edge: string; questions: string[]; details?: Question[] } | null;
  created: number;
  /** When it reached done or cancelled, in Unix seconds. */
  finished?: number | null;
  /** The user chose to carry on past the plan limits. */
  waived?: boolean;
  log?: LogEntry[];
}

export interface RunSummary {
  id: string;
  /** The flow this is an execution of. It may no longer be saved. */
  flowId: string;
  flowName: string;
  goal: string;
  status: RunStatus;
  created: number;
  /** Everything the agents read and wrote, cache included. */
  tokens: number;
  /** Dollars charged by OpenRouter for the run's agents on it. */
  costUsd?: number;
}

/** A pinned OpenRouter model a flow's agent may use. */
export interface FlowModel {
  account: string;
  accountName: string;
  id: string;
  /** The CLI that runs it unless the agent is told otherwise. */
  driver: Engine;
  priceIn: number | null;
  priceOut: number | null;
  tools: boolean;
  /** Called directly, with no CLI and no tools: every model of a plain OpenAI-compatible endpoint is. */
  direct: boolean;
}

/** One turn of one agent, in brief. The full record is fetched when the turn is opened. */
export interface Turn {
  n: number;
  node: string;
  attempt: number;
  /** How many send-backs had happened when it started. */
  round: number;
  started: number;
  ended: number;
  engine: 'claude' | 'codex' | 'direct';
  model: string;
  account?: string | null;
  tokens: Tokens;
  costUsd: number;
  credits: number;
  session?: string | null;
  failed?: string | null;
  decision?: string | null;
  commit?: string | null;
  effort?: string | null;
  reason?: string | null;
}

export type TraceKind = 'prompt' | 'say' | 'tool' | 'result' | 'note';

export interface TraceEntry {
  kind: TraceKind;
  title: string;
  body: string;
}

/** What a turn left besides its summary. */
export interface TurnDetail {
  n: number;
  output: unknown;
  error: string | null;
  trace: TraceEntry[];
}

export interface Models {
  claude: string[];
  codex: string[];
  openrouter: FlowModel[];
  /** There is a web account, so agents can be given web access. */
  web?: boolean;
}

export interface FlowLists {
  saved: Flow[];
  starters: Flow[];
}

export interface Window {
  used_pct: number;
  resets_at: number | null;
}

export interface ToolUsage {
  five_hour: Window | null;
  seven_day: Window | null;
  error: string | null;
}

export interface UsageSnapshot {
  claude: ToolUsage;
  codex: ToolUsage;
  /** One per OpenRouter account with a key. */
  openrouter?: OrUsage[];
}
