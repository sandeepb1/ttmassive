// Helper agents (Claude's subagents, Codex's spawned threads) as the browser sees them.
// Pure, so node can test it.

export interface SubAgent {
  id: string;
  /** Claude's agent type (`Explore`), or `codex`. */
  kind: string;
  description: string;
  /** The id of the tool call in the parent conversation that started it. */
  tool_use_id: string | null;
  depth: number;
  /** The helper that started this one, when it was not the main conversation. */
  parent: string | null;
  running: boolean;
  /** Unix seconds. */
  started: number | null;
  last: number | null;
  tools: number;
  last_action: string | null;
  result: string | null;
  has_log: boolean;
}

/** The tool items that start a helper. */
export const isAgentTool = (tool: string): boolean => tool === 'Agent' || tool === 'Task';

/** The helper that the tool call `itemId` started. */
export function agentForItem(agents: SubAgent[], itemId: string): SubAgent | undefined {
  return agents.find((a) => a.tool_use_id === itemId);
}

/** Helpers started by `parent` (null: by the main conversation). */
export function childrenOf(agents: SubAgent[], parent: string | null): SubAgent[] {
  return agents.filter((a) => a.parent === parent);
}

/** Helpers laid out as a tree: each one followed by the ones it started, with its depth in the tree. */
export function asTree(agents: SubAgent[]): { agent: SubAgent; level: number }[] {
  const out: { agent: SubAgent; level: number }[] = [];
  const seen = new Set<string>();
  const walk = (parent: string | null, level: number) => {
    for (const a of childrenOf(agents, parent)) {
      if (seen.has(a.id)) continue;
      seen.add(a.id);
      out.push({ agent: a, level });
      walk(a.id, level + 1);
    }
  };
  walk(null, 0);
  // A helper whose parent is not in the list (it was trimmed) still shows, at the top.
  for (const a of agents) if (!seen.has(a.id)) out.push({ agent: a, level: 0 });
  return out;
}

export type AgentState = 'starting' | 'running' | 'done';

export function stateOf(a: SubAgent): AgentState {
  if (a.running) return a.has_log ? 'running' : 'starting';
  return 'done';
}

/** How long it has been going (running) or took (done), in seconds. */
export function elapsedSecs(a: SubAgent, nowSecs: number): number | null {
  if (a.started === null) return null;
  const end = a.running ? nowSecs : (a.last ?? nowSecs);
  return Math.max(0, end - a.started);
}

export function fmtElapsed(secs: number): string {
  const s = Math.floor(secs);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`;
  return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}m`;
}

/** What a card or row says about a helper's work so far. */
export function workLine(a: SubAgent): string {
  const calls = `${a.tools} tool call${a.tools === 1 ? '' : 's'}`;
  return a.last_action ? `${calls} · ${a.last_action}` : calls;
}
