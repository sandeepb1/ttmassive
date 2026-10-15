// Orchestrator mode. Holds the state: the flow being edited, the run being watched, and what is on screen.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import type { RecentDir } from '../dirs';
import { orch, watchRun } from './api';
import { Icon, ROLE_ICON } from './bits';
import { GrillDialog, NewRunDialog, SuggestDialog } from './Dialogs';
import { FlowCanvas, useViewportCenter } from './FlowCanvas';
import { Inspector } from './Inspector';
import { NodeList } from './NodeList';
import { RunBar } from './RunBar';
import { Sidebar } from './Sidebar';
import { UsageMeters } from './UsageMeters';
import { ROLES, ROLE_INFO, emptyFlow, flowProblem, gauges, newNode, tidy } from './model';
import type { Flow, FlowLists, Models, Role, Run, RunSummary, UsageSnapshot } from './types';

const PHONE = '(max-width: 699px)';
const WIDE = '(min-width: 1100px)';

function useMedia(query: string): boolean {
  const [on, setOn] = useState(() => matchMedia(query).matches);
  useEffect(() => {
    const mq = matchMedia(query);
    const f = () => setOn(mq.matches);
    mq.addEventListener('change', f);
    return () => mq.removeEventListener('change', f);
  }, [query]);
  return on;
}

const newFlowId = () => `f${Date.now().toString(36)}`;
const cloneFlow = (f: Flow): Flow => ({ ...structuredClone(f), id: newFlowId() });

function Palette({ count, onAdd }: { count: number; onAdd: (role: Role, at: { x: number; y: number }) => void }) {
  const center = useViewportCenter();
  return (
    <div className="mc-palette" role="toolbar" aria-label="Add an agent">
      {ROLES.map((role) => (
        <button key={role} type="button" className="mc-btn" title={ROLE_INFO[role].blurb} onClick={() => onAdd(role, center(count))}>
          <Icon name={ROLE_ICON[role]} /> {ROLE_INFO[role].label}
        </button>
      ))}
    </div>
  );
}

export function App({ initialRun, onExit, onSession, onRoute }: { initialRun: string | null; onExit: () => void; onSession: (id: string) => void; onRoute: (runId: string | null) => void }) {
  const wide = useMedia(WIDE);
  const phone = useMedia(PHONE);

  const [models, setModels] = useState<Models>({ claude: ['opus', 'sonnet', 'haiku'], codex: [] });
  const [lists, setLists] = useState<FlowLists>({ saved: [], starters: [] });
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [usage, setUsage] = useState<UsageSnapshot | null>(null);
  const [recents, setRecents] = useState<RecentDir[]>([]);

  const [flow, setFlowState] = useState<Flow>(() => emptyFlow(newFlowId(), 'New flow'));
  // The latest flow, readable at once: an edit that needs to know the result (a new agent's id) cannot wait for a render.
  const flowRef = useRef(flow);
  const setFlow = useCallback((f: Flow) => {
    flowRef.current = f;
    setFlowState(f);
  }, []);
  const [dirty, setDirty] = useState(false);
  const [runId, setRunId] = useState<string | null>(initialRun);
  const [run, setRun] = useState<Run | null>(null);
  const [link, setLink] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);

  const [sideOpen, setSideOpen] = useState(false);
  const [inspOpen, setInspOpen] = useState(false);
  const [view, setView] = useState<'canvas' | 'list'>('canvas');
  const [dialog, setDialog] = useState(false);
  const [suggesting, setSuggesting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [thinking, setThinking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [fitToken, setFitToken] = useState(0);

  const say = useCallback((m: string) => {
    setToast(m);
    window.setTimeout(() => setToast((t) => (t === m ? null : t)), 2800);
  }, []);

  // ------------------------------------------------------------------ loading
  const refreshLists = useCallback(async () => {
    const r = await orch.flows();
    if (r.ok) setLists(r.data);
  }, []);
  const refreshRuns = useCallback(async () => {
    const r = await orch.runs();
    if (r.ok) setRuns(r.data);
  }, []);

  useEffect(() => {
    void (async () => {
      const [m, f, rs, u, d] = await Promise.all([orch.models(), orch.flows(), orch.runs(), orch.usage(), orch.recentDirs()]);
      if (m.ok) setModels(m.data);
      if (f.ok) {
        setLists(f.data);
        if (!initialRun && f.data.starters[0]) setFlow(cloneFlow(f.data.starters[0]));
      }
      if (rs.ok) setRuns(rs.data);
      if (u.ok) setUsage(u.data);
      setRecents(d);
    })();
    // Once, when the mode opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const id = window.setInterval(() => {
      void orch.usage().then((u) => u.ok && setUsage(u.data));
      void refreshRuns();
    }, 15000);
    return () => window.clearInterval(id);
  }, [refreshRuns]);

  // ------------------------------------------------------------------ following a run
  // A link to another run, or Back and Forward between them, changes the address while the mode is open.
  useEffect(() => {
    const follow = () => {
      const m = /^#orch\/([A-Za-z0-9_-]+)$/.exec(location.hash);
      if (!m) return;
      setRunId((now) => (now === m[1] ? now : m[1]!));
      setSelected(null);
    };
    window.addEventListener('hashchange', follow);
    return () => window.removeEventListener('hashchange', follow);
  }, []);

  useEffect(() => {
    onRoute(runId);
    if (!runId) {
      setRun(null);
      setLink(true);
      return;
    }
    let alive = true;
    void orch.getRun(runId).then((r) => {
      if (!alive) return;
      if (r.ok) setRun(r.data);
      else {
        setError(r.error);
        setRunId(null);
      }
    });
    const stop = watchRun(
      runId,
      (r) => alive && setRun(r),
      (up) => alive && setLink(up),
    );
    return () => {
      alive = false;
      stop();
    };
    // onRoute is stable from the caller.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runId]);

  useEffect(() => {
    if (run) void refreshRuns();
  }, [run?.status.state, refreshRuns]); // eslint-disable-line react-hooks/exhaustive-deps

  const act = useCallback(
    async <T,>(call: () => Promise<{ ok: true; data: T } | { ok: false; error: string }>, then?: (d: T) => void) => {
      setBusy(true);
      setError(null);
      const r = await call();
      setBusy(false);
      if (r.ok) then?.(r.data);
      else setError(r.error);
      return r.ok;
    },
    [],
  );

  // ------------------------------------------------------------------ grilling
  const doGrill = useCallback(
    async (answers: string[]) => {
      if (!run) return;
      setThinking(true);
      setError(null);
      const r = await orch.grill(run.id, answers);
      setThinking(false);
      if (r.ok) setRun(r.data);
      else setError(r.error);
    },
    [run],
  );

  // The first round of questions starts by itself, once for each round.
  const asked = useRef('');
  useEffect(() => {
    if (!run || run.status.state !== 'grilling' || !run.grill || thinking) return;
    const g = run.grill;
    if (g.pending.length || g.brief) return;
    const key = `${run.id}:${g.round}`;
    if (asked.current === key) return;
    asked.current = key;
    void doGrill([]);
  }, [run, thinking, doGrill]);

  // ------------------------------------------------------------------ the flow being edited
  const editFlow = useCallback(
    (update: (f: Flow) => Flow) => {
      setFlow(update(flowRef.current));
      setDirty(true);
    },
    [setFlow],
  );
  const addNode = useCallback(
    (role: Role, at: { x: number; y: number }) => {
      const n = newNode(flowRef.current, role, models, at);
      editFlow((f) => ({ ...f, nodes: [...f.nodes, n] }));
      setSelected(n.id);
      setInspOpen(true);
    },
    [editFlow, models],
  );

  const shown = run ? run.flow : flow;
  const editable = !run;
  const problem = useMemo(() => flowProblem(flow), [flow]);
  const selectedNode = shown.nodes.find((n) => n.id === selected) ?? null;
  const showList = phone || view === 'list';
  const meters = useMemo(() => gauges(shown, run, usage), [shown, run, usage]);

  const openFlow = (f: Flow, starter: boolean) => {
    setRunId(null);
    setFlow(starter ? cloneFlow(f) : structuredClone(f));
    setDirty(starter);
    setSelected(null);
    setSideOpen(false);
  };
  const openRun = (id: string) => {
    setRunId(id);
    setSelected(null);
    setSideOpen(false);
  };
  const select = (id: string | null) => {
    setSelected(id);
    if (id && !wide) setInspOpen(true);
  };
  const save = () =>
    void act(() => orch.saveFlow(flow), () => {
      setDirty(false);
      say('Flow saved');
      void refreshLists();
    });
  const removeFlow = (f: Flow) => {
    if (!confirm(`Delete the flow “${f.name}”?`)) return;
    void act(() => orch.deleteFlow(f.id), () => void refreshLists());
  };
  const suggest = (goal: string) =>
    void act(
      () => orch.suggest(goal),
      (f) => {
        setRunId(null);
        setFlow(f);
        setDirty(true);
        setSelected(null);
        setSuggesting(false);
        setSideOpen(false);
        window.setTimeout(() => setFitToken((t) => t + 1), 0);
        say('Here is a flow for it. Change anything, then run it.');
      },
    );
  const popOut = async (runIdNow: string, node: string) => {
    const r = await orch.popOut(runIdNow, node);
    if (r.ok) {
      say('Opening the live session…');
      onSession(r.data.session);
    } else say(r.error);
  };
  const copyText = (t: string) => void navigator.clipboard?.writeText(t).then(() => say('Copied'), () => say('Could not copy'));

  const createRun = (goal: string, project: string, skipGrill: boolean) =>
    void act(
      () => orch.createRun({ flow, goal, project, skipGrill }),
      (r) => {
        setDialog(false);
        setRun(r);
        setRunId(r.id);
        setSelected(null);
      },
    );

  return (
    <>
      <header className="mc-top">
        <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Back to sessions" onClick={onExit}>
          <Icon name="back" />
        </button>
        {!wide && (
          <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Runs and flows" onClick={() => setSideOpen((o) => !o)}>
            <Icon name="menu" />
          </button>
        )}
        <div className="mc-brand">
          <span className="mc-brand-mark">
            <Icon name="workflow" />
          </span>
          <span className="mc-brand-text">Orchestrator</span>
        </div>
        <input className="mc-flow-name" aria-label="Flow name" value={shown.name} disabled={!editable} maxLength={60} onChange={(e) => editFlow((f) => ({ ...f, name: e.target.value }))} />
        {editable && dirty && (
          <span className="mc-pill mc-hide-narrow" data-tone="warn">
            unsaved
          </span>
        )}
        <span className="mc-spacer" />
        <UsageMeters gauges={meters} />
        <div className="mc-top-actions">
          {editable ? (
            <>
              {!phone && (
                <button type="button" className="mc-btn mc-hide-narrow" onClick={() => (editFlow(tidy), window.setTimeout(() => setFitToken((t) => t + 1), 0))} title="Line the agents up left to right">
                  <Icon name="grid" /> Tidy
                </button>
              )}
              <button type="button" className="mc-btn mc-hide-narrow" disabled={!dirty || !!problem || busy} onClick={save} title={problem ?? 'Save this flow'}>
                <Icon name="save" /> Save
              </button>
              <button type="button" className="mc-btn mc-btn-primary" disabled={!!problem} onClick={() => setDialog(true)} title={problem ?? 'Start a run of this flow'}>
                <Icon name="play" /> Run
              </button>
            </>
          ) : (
            <button
              type="button"
              className="mc-btn"
              onClick={() => {
                setFlow(cloneFlow(run!.flow));
                setDirty(true);
                setRunId(null);
              }}
              title="Edit a copy of this flow"
            >
              <Icon name="edit" /> <span className="mc-hide-narrow">Edit a copy</span>
            </button>
          )}
          {!phone && (
            <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={showList ? 'Show the chart' : 'Show a list'} onClick={() => setView(showList ? 'canvas' : 'list')}>
              <Icon name={showList ? 'columns' : 'todo'} />
            </button>
          )}
          {!wide && (
            <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Details" onClick={() => setInspOpen((o) => !o)}>
              <Icon name="panel-right" />
            </button>
          )}
        </div>
      </header>

      <div className="mc-body">
        <aside className="mc-side" data-open={sideOpen} aria-label="Runs and flows">
          <Sidebar
            runs={runs}
            flows={lists}
            runId={runId}
            flowId={flow.id}
            onNewRun={() => (runId && setRunId(null), setSideOpen(false), setDialog(true))}
            onOpenRun={openRun}
            onOpenFlow={openFlow}
            onNewFlow={() => openFlow(emptyFlow(newFlowId(), 'New flow'), false)}
            onSuggest={() => (setError(null), setSuggesting(true))}
            onDuplicateFlow={(f) => openFlow({ ...f, name: `${f.name} copy`.slice(0, 60) }, true)}
            onDeleteFlow={removeFlow}
          />
        </aside>

        <main className="mc-stage" data-list={showList}>
          {run && <RunBar run={run} busy={busy} link={link} onApprove={(n) => void act(() => orch.approve(run.id, n), setRun)} onResume={() => void act(() => orch.resume(run.id), setRun)} onCancel={() => void act(() => orch.cancel(run.id), setRun)} onCopy={copyText} />}
          {showList ? (
            <NodeList flow={shown} run={run} selected={selected} onSelect={select} />
          ) : (
            <ReactFlowProvider>
              <FlowCanvas flow={shown} run={run} selected={selected} editable={editable} onFlow={editFlow} onSelect={select} onProblem={say} fitToken={fitToken} viewKey={run ? `run-${run.id}` : flow.id} />
              {editable && <Palette count={flow.nodes.length} onAdd={addNode} />}
            </ReactFlowProvider>
          )}
          {editable && flow.nodes.length === 0 && (
            <div className="mc-empty" style={{ position: 'absolute', inset: '32% 0 auto', zIndex: 3 }}>
              <p style={{ margin: '0 0 0.75rem' }}>An empty flow. Add agents from the bar below, open a starter, or</p>
              <button type="button" className="mc-btn" onClick={() => (setError(null), setSuggesting(true))}>
                <Icon name="wand" /> Suggest a flow
              </button>
            </div>
          )}
          {error && !dialog && run?.status.state !== 'grilling' && (
            <p className="mc-error" role="alert" style={{ position: 'absolute', left: '0.75rem', bottom: '0.75rem', right: '0.75rem', margin: 0, zIndex: 6 }}>
              {error}
            </p>
          )}
          {toast && (
            <div className="mc-toast" role="status">
              {toast}
            </div>
          )}
          {suggesting && <SuggestDialog busy={busy} error={error} onSuggest={suggest} onClose={() => (setSuggesting(false), setError(null))} />}
          {dialog && <NewRunDialog flow={flow} recents={recents} busy={busy} error={error} onCreate={createRun} onClose={() => (setDialog(false), setError(null))} />}
          {run?.status.state === 'grilling' && (
            <GrillDialog
              run={run}
              thinking={thinking}
              error={error}
              onAnswer={(a) => void doGrill(a)}
              onStart={(brief) => void act(() => orch.start(run.id, brief), setRun)}
              onStop={() => void act(() => orch.cancel(run.id), setRun)}
            />
          )}
        </main>

        <aside className="mc-inspector" data-open={inspOpen} aria-label="Details">
          <Inspector
            flow={shown}
            node={selectedNode}
            run={run}
            models={models}
            editable={editable}
            goal={run?.goal ?? shown.name}
            onNode={(id, patch) => editFlow((f) => ({ ...f, nodes: f.nodes.map((n) => (n.id === id ? { ...n, ...patch } : n)) }))}
            onFlow={(patch) => editFlow((f) => ({ ...f, ...patch }))}
            onDelete={(id) => (editFlow((f) => ({ ...f, nodes: f.nodes.filter((n) => n.id !== id), edges: f.edges.filter((e) => e.from !== id && e.to !== id) })), setSelected(null))}
            onWritePrompt={async (n) => {
              const r = await orch.nodePrompt(n.role, n.name, run?.goal ?? shown.name);
              if (!r.ok) {
                say(r.error);
                return null;
              }
              return r.data.systemPrompt;
            }}
            onPopOut={(n) => (run ? popOut(run.id, n.id) : Promise.resolve())}
            onClose={() => (setSelected(null), setInspOpen(false))}
          />
        </aside>
      </div>
    </>
  );
}
