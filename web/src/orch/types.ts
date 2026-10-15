// The server's orchestrator JSON, as the page sees it. Field names are the ones src/orch/*.rs write.

export type Role = 'orchestrator' | 'researcher' | 'search' | 'analyst' | 'judge' | 'builder';
export type Engine = 'claude' | 'codex';
export type EdgeKind = 'forward' | 'retry';

export interface FlowNode {
  id: string;
  name: string;
  role: Role;
  engine: Engine;
  model: string;
  systemPrompt: string;
  checkpoint: boolean;
  tokenCap?: number | null;
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
  recommended: string;
}

export interface Grill {
  round: number;
  history: { question: string; answer: string }[];
  pending: Question[];
  brief: string | null;
}

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
}

export interface RunSummary {
  id: string;
  flowName: string;
  goal: string;
  status: RunStatus;
  created: number;
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
