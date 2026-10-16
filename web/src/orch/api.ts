// Calls to /api/orch. A failure comes back as a message to show; a 401 reloads the page to the login.

import { fetchRecentDirs, type RecentDir } from '../dirs';
import type { TurnDetail, Budget, Capability, CustomRole, Flow, FlowLists, Models, Output, Role, RepoState, RoleDef, Run, RunSummary, UsageSnapshot } from './types';

export type Result<T> = { ok: true; data: T } | { ok: false; error: string };

const signedOut = () => location.reload();

async function send<T>(method: string, url: string, body?: unknown): Promise<Result<T>> {
  let r: Response;
  try {
    r = await fetch(url, {
      method,
      cache: 'no-store',
      headers: body === undefined ? undefined : { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    return { ok: false, error: 'The server cannot be reached' };
  }
  if (r.status === 401) {
    signedOut();
    return { ok: false, error: 'Signed out' };
  }
  if (!r.ok) {
    const text = (await r.text().catch(() => '')).trim();
    return { ok: false, error: text || `The server answered ${r.status}` };
  }
  if (r.status === 204) return { ok: true, data: undefined as T };
  try {
    return { ok: true, data: (await r.json()) as T };
  } catch {
    return { ok: false, error: 'The reply could not be read' };
  }
}

const base = '/api/orch';
const run = (id: string) => `${base}/runs/${encodeURIComponent(id)}`;

export const orch = {
  models: () => send<Models>('GET', `${base}/models`),
  flows: () => send<FlowLists>('GET', `${base}/flows`),
  saveFlow: (f: Flow) => send<void>('PUT', `${base}/flows/${encodeURIComponent(f.id)}`, f),
  deleteFlow: (id: string) => send<void>('DELETE', `${base}/flows/${encodeURIComponent(id)}`),
  roles: () => send<RoleDef[]>('GET', `${base}/roles`),
  nodePrompt: (role: Role, custom: CustomRole | null | undefined, name: string, goal: string) =>
    send<{ systemPrompt: string }>('POST', `${base}/node-prompt`, { role, custom: custom ?? null, name, goal }),
  customRole: (description: string) =>
    send<{ label: string; capability: Capability; output: Output; instructions: string }>('POST', `${base}/custom-role`, { description }),
  runs: () => send<RunSummary[]>('GET', `${base}/runs`),
  getRun: (id: string) => send<Run>('GET', run(id)),
  createRun: (req: { flow: Flow; goal: string; project: string; skipGrill: boolean; readLinks?: boolean; docs?: string[] }) => send<Run>('POST', `${base}/runs`, req),
  grill: (id: string, answers: string[]) => send<Run>('POST', `${run(id)}/grill`, { answers }),
  start: (id: string, brief?: string) => send<Run>('POST', `${run(id)}/start`, { brief }),
  /** The full record of one turn: what the agent was asked, said, ran, and handed on. */
  turn: (id: string, n: number) => send<TurnDetail>('GET', `${run(id)}/turns/${n}`),
  approve: (id: string, node: string) => send<Run>('POST', `${run(id)}/approve`, { node }),
  /** The user's answers to the questions a paused run is waiting on. */
  answers: (id: string, answers: string[]) => send<Run>('POST', `${run(id)}/answers`, { answers }),
  /** `waive` also stops the plan limits being checked again for this run. */
  resume: (id: string, waive = false) => send<Run>('POST', `${run(id)}/resume`, waive ? { waive: true } : undefined),
  limits: () => send<Budget>('GET', `${base}/limits`),
  setLimits: (b: Budget) => send<Budget>('PUT', `${base}/limits`, b),
  projectState: (path: string) => send<{ state: RepoState }>('GET', `${base}/project?path=${encodeURIComponent(path)}`),
  projectInit: (path: string) => send<{ state: RepoState }>('POST', `${base}/project/init`, { path }),
  deleteRun: (id: string) => send<void>('DELETE', run(id)),
  cancel: (id: string) => send<Run>('POST', `${run(id)}/cancel`),
  /** Change a flow as asked, with Claude or Codex. Nothing is saved. */
  editFlow: (flow: Flow, request: string, engine: 'claude' | 'codex') => send<{ flow: Flow; summary: string }>('POST', `${base}/flows/edit`, { flow, request, engine }),
  suggest: (goal: string) => send<Flow & { notes?: string[] }>('POST', `${base}/flows/suggest`, { goal }),
  popOut: (id: string, node: string) => send<{ session: string }>('POST', `${run(id)}/popout`, { node }),
  usage: () => send<UsageSnapshot>('GET', '/api/usage'),
  recentDirs: (): Promise<RecentDir[]> => fetchRecentDirs(),
};

/**
 * Follow one run live. `onRun` gets the whole run now and after every change. If the socket drops it is
 * reopened with a growing pause, and the run is fetched again so nothing is missed meanwhile.
 * Returns a function that stops it.
 */
export function watchRun(id: string, onRun: (r: Run) => void, onLink: (up: boolean) => void): () => void {
  let ws: WebSocket | null = null;
  let timer = 0;
  let tries = 0;
  let stopped = false;
  const open = () => {
    if (stopped) return;
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
    ws = new WebSocket(`${scheme}://${location.host}${base}/events?run=${encodeURIComponent(id)}`);
    ws.onopen = () => {
      tries = 0;
      onLink(true);
    };
    ws.onmessage = (m) => {
      try {
        onRun(JSON.parse(String(m.data)) as Run);
      } catch {
        /* a frame that is not a run is ignored */
      }
    };
    ws.onclose = () => {
      onLink(false);
      if (stopped) return;
      timer = window.setTimeout(open, Math.min(1000 * 2 ** tries++, 15000));
    };
  };
  open();
  return () => {
    stopped = true;
    window.clearTimeout(timer);
    ws?.close();
  };
}

