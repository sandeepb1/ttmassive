// The server's orchestrator JSON, as the page sees it. Field names are the ones src/orch/*.rs write.

/** A built-in role's id, or `custom`. The server's catalog (`/api/orch/roles`) says which exist. */
export type Role = string;
export type Capability = 'read' | 'inspect' | 'write';
export type Output = 'plan' | 'report' | 'change' | 'verdict';
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
  checkpoint: boolean;
  prefer: Engine;
  claudeModel: string;
  codexModel?: string | null;
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
  systemPrompt: string;
  checkpoint: boolean;
  tokenCap?: number | null;
  /** Never switch this agent to a cheaper model. */
  keepModel?: boolean;
  x: number;
  y: number;
}

export interface FlowEdge {
  id: string;
  from: string;
  to: string;
  kind: EdgeKind;
}

export interface Budget {
  claudePct: number;
  codexPct: number;
  stopWindowPct: number;
}

export interface Flow {
  id: string;
  name: string;
  nodes: FlowNode[];
  edges: FlowEdge[];
  maxRetries: number;
  budget: Budget;
  /** Give agents that have not started yet a cheaper model as the plan windows fill up. */
  downshift?: boolean;
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
  transcript: string[];
  /** The agent's conversation id after its latest finished turn; present once it has finished one. */
  session?: string | null;
  /** The model it actually ran on: the flow's, or a cheaper one. */
  modelUsed?: string | null;
  /** Times a turn got far less from the prompt cache than the one before it. */
  cacheBreaks?: number;
}

export type Pause =
  | { kind: 'approval'; node: string }
  | { kind: 'retriesExhausted'; node: string }
  | { kind: 'budget'; message: string }
  | { kind: 'failed'; node: string; message: string };

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
  created: number;
  /** When it reached done or cancelled, in Unix seconds. */
  finished?: number | null;
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
}

export interface Models {
  claude: string[];
  codex: string[];
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
}
