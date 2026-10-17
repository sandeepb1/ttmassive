// Orchestrator mode. Holds the state: the flow being edited, the run being watched, and what is on screen.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import type { RecentDir } from '../dirs';
import { orch, watchRun } from './api';
import { Icon, iconFor } from './bits';
import { CustomRoleDialog, GrillDialog, NewRunDialog, QuestionsDialog, RerunProblemDialog, SuggestDialog, type RoleDraft } from './Dialogs';
import { Assistant } from './Assistant';
import { ContextMenu, type MenuItem, type MenuState } from './ContextMenu';
import { FlowCanvas, useViewportCenter } from './FlowCanvas';
import { LogPanel, LogToggle } from './LogPanel';
import { Timeline } from './Timeline';
import { Inspector } from './Inspector';
import { NodeList } from './NodeList';
import { RunBar } from './RunBar';
import { Sidebar } from './Sidebar';
import { UsageMeters } from './UsageMeters';
import { CAPABILITY_TEXT, STAGES, emptyFlow, flowEntries, flowProblem, gauges, getCatalog, offered, orGauges, rerunRequest, isFinished, newCustomNode, newNode, roleDef, setCatalog, tidy, type FlowEntry } from './model';
import { DEFAULT_LIMITS, type Budget, type Flow, type FlowLists, type Models, type Role, type RoleDef, type Run, type RunSummary, type UsageSnapshot } from './types';

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

/** The agents that can be added, grouped by where they sit in a flow, with the ones most flows need first. */
function AddAgent({ roles, count, onAdd, onCustom }: { roles: RoleDef[]; count: number; onAdd: (role: Role, at: { x: number; y: number }) => void; onCustom: () => void }) {
  const center = useViewportCenter();
  const [open, setOpen] = useState(false);
  const shown = offered(roles);
  const core = shown.filter((r) => r.core);
  // With every offered role in the toolbar there is no list to open: the one extra is a role of your own.
  const hasMore = shown.some((r) => !r.core);
  const add = (role: Role) => {
    onAdd(role, center(count));
    setOpen(false);
  };
  useEffect(() => {
    if (!open) return;
    const key = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [open]);
  return (
    <div className="mc-palette" role="toolbar" aria-label="Add an agent">
      {open && (
        <div className="mc-addpanel" role="menu" aria-label="All roles">
          {STAGES.map((st) => {
            const list = shown.filter((r) => r.stage === st.id);
            if (!list.length) return null;
            return (
              <div key={st.id} className="mc-addgroup">
                <label className="mc-label">{st.label}</label>
                {list.map((r) => (
                  <button key={r.id} type="button" role="menuitem" className="mc-addrow" onClick={() => add(r.id)}>
                    <span className="mc-role">
                      <Icon name={iconFor({ role: r.id })} />
                    </span>
                    <span className="mc-addtext">
                      <span className="mc-addname">{r.label}</span>
                      <span className="mc-addblurb">
                        {r.blurb} · {CAPABILITY_TEXT[r.capability].toLowerCase()}
                      </span>
                    </span>
                  </button>
                ))}
              </div>
            );
          })}
          <div className="mc-addgroup">
            <label className="mc-label">Your own</label>
            <button
              type="button"
              role="menuitem"
              className="mc-addrow"
              onClick={() => {
                setOpen(false);
                onCustom();
              }}
            >
              <span className="mc-role">
                <Icon name="wand" />
              </span>
              <span className="mc-addtext">
                <span className="mc-addname">Custom role…</span>
                <span className="mc-addblurb">Describe a job; Sonnet writes the instructions</span>
              </span>
            </button>
          </div>
        </div>
      )}
      {core.map((r) => (
        <button key={r.id} type="button" className="mc-btn" title={r.blurb} onClick={() => add(r.id)}>
          <Icon name={iconFor({ role: r.id })} /> {r.label}
        </button>
      ))}
      {hasMore ? (
        <button type="button" className="mc-btn" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          <Icon name="plus" /> More roles
        </button>
      ) : (
        <button type="button" className="mc-btn" title="Describe a job; Sonnet writes the instructions" onClick={onCustom}>
          <Icon name="wand" /> Custom role
        </button>
      )}
    </div>
  );
}

export function App({ initialRun, onExit, onSession, onRoute }: { initialRun: string | null; onExit: () => void; onSession: (id: string) => void; onRoute: (runId: string | null) => void }) {
  const wide = useMedia(WIDE);
  const phone = useMedia(PHONE);

  const [models, setModels] = useState<Models>({ claude: ['opus', 'sonnet', 'haiku'], codex: [], openrouter: [] });
  const [lists, setLists] = useState<FlowLists>({ saved: [], starters: [] });
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [usage, setUsage] = useState<UsageSnapshot | null>(null);
  const [limits, setLimitsState] = useState<Budget>(DEFAULT_LIMITS);
  const [recents, setRecents] = useState<RecentDir[]>([]);

  const [flow, setFlowState] = useState<Flow>(() => emptyFlow(newFlowId(), 'New flow'));
  // The latest flow, readable at once: an edit that needs to know the result (a new agent's id) cannot wait for a render.
  const flowRef = useRef(flow);
  const setFlow = useCallback((f: Flow) => {
    flowRef.current = f;
    setFlowState(f);
  }, []);
  const [dirty, setDirty] = useState(false);
  // Whether the flow has been changed since it was opened or saved: leaving it then asks first.
  const edited = useRef(false);
  // The sidebar's level: the list of flows, or the open flow with its executions.
  const [nav, setNav] = useState<'flows' | 'flow'>('flow');
  const [loaded, setLoaded] = useState(false);
  const [runId, setRunId] = useState<string | null>(initialRun);
  const [run, setRun] = useState<Run | null>(null);
  const [link, setLink] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const [selectedEdge, setSelectedEdge] = useState<string | null>(null);
  const [answering, setAnswering] = useState(false);

  const [sideOpen, setSideOpen] = useState(false);
  const [inspOpen, setInspOpen] = useState(false);
  const [view, setView] = useState<'canvas' | 'list'>('canvas');
  // An execution opens on its timeline; the chart is one tap away. A flow being designed has only the chart.
  const [runView, setRunView] = useState<'timeline' | 'chart'>('timeline');
  // A run that could not be started again: why, and where it was.
  const [rerunProblem, setRerunProblem] = useState<{ id: string; message: string; project: string; git: boolean } | null>(null);
  const [dialog, setDialog] = useState(false);
  /** The goal the run dialog starts with: the text a flow was just designed from, when the person chose Design and run. */
  const [runGoal, setRunGoal] = useState('');
  const [suggesting, setSuggesting] = useState(false);
  const [roles, setRoles] = useState<RoleDef[]>(getCatalog());
  const [customOpen, setCustomOpen] = useState(false);
  const [draft, setDraft] = useState<RoleDraft | null>(null);
  const [designing, setDesigning] = useState(false);
  const [customError, setCustomError] = useState<string | null>(null);
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
      const [m, f, rs, u, d, ro, li] = await Promise.all([orch.models(), orch.flows(), orch.runs(), orch.usage(), orch.recentDirs(), orch.roles(), orch.limits()]);
      if (li.ok) setLimitsState(li.data);
      if (m.ok) setModels(m.data);
      if (ro.ok) {
        setCatalog(ro.data);
        setRoles(ro.data);
      }
      if (f.ok) {
        setLists(f.data);
        // The editor starts on the flow used most recently, so what is on screen is one of the flows in the list. With
        // no flows yet it starts on a template, not a blank page.
        const latest = flowEntries(f.data.saved, rs.ok ? rs.data : []).find((e) => e.saved);
        const open = latest && f.data.saved.find((x) => x.id === latest.id);
        if (open) (setFlow(structuredClone(open)), setDirty(false));
        else if (f.data.starters[0]) setFlow(cloneFlow(f.data.starters[0]));
      }
      if (rs.ok) setRuns(rs.data);
      if (u.ok) setUsage(u.data);
      setRecents(d);
      // Someone with flows or past executions starts at their list; a new user starts in the editor.
      if (!initialRun && ((f.ok && f.data.saved.length) || (rs.ok && rs.data.length))) setNav('flows');
      setLoaded(true);
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
      if (m) setRunId((now) => (now === m[1] ? now : m[1]!));
      else if (location.hash === '#orch/') setRunId(null); // the address with no run is the editor
      else return;
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

  // An execution belongs to a flow. Opening one (from a link, say) opens its flow too: the saved one if there is
  // one, otherwise the flow as it was then, which becomes a flow again when it is saved.
  useEffect(() => {
    if (!run || !loaded || run.flow.id === flowRef.current.id) return;
    const savedFlow = lists.saved.find((f) => f.id === run.flow.id);
    setFlow(structuredClone(savedFlow ?? run.flow));
    setDirty(!savedFlow);
    edited.current = false;
    setNav('flow');
  }, [run?.id, loaded, lists.saved, setFlow]); // eslint-disable-line react-hooks/exhaustive-deps

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
      edited.current = true;
    },
    [setFlow],
  );
  const addNode = useCallback(
    (role: Role, at: { x: number; y: number }) => {
      const def = roleDef(role);
      if (!def) return;
      const n = newNode(flowRef.current, def, models, at);
      editFlow((f) => ({ ...f, nodes: [...f.nodes, n] }));
      setSelected(n.id);
      setInspOpen(true);
    },
    [editFlow, models],
  );

  const entries = useMemo(() => flowEntries(lists.saved, runs), [lists.saved, runs]);
  const isSaved = lists.saved.some((f) => f.id === flow.id);
  const shown = run ? run.flow : flow;
  const editable = !run;
  const problem = useMemo(() => flowProblem(flow), [flow]);
  const selectedNode = shown.nodes.find((n) => n.id === selected) ?? null;
  const selectedEdgeObj = selectedNode ? null : (shown.edges.find((e) => e.id === selectedEdge) ?? null);
  const showTimeline = !!run && runView === 'timeline';
  const showList = !showTimeline && (phone || view === 'list');
  const meters = useMemo(() => gauges(shown, run, usage, limits), [shown, run, usage, limits]);
  const apiMeters = useMemo(() => orGauges(shown, run, usage, limits), [shown, run, usage, limits]);

  /** Leaving a flow that has been changed asks first; returns whether to go on. */
  const mayLeave = () => !edited.current || confirm(`Discard your unsaved changes to “${flow.name}”?`);
  const fit = () => window.setTimeout(() => setFitToken((t) => t + 1), 0);
  /** Show a flow in the editor: a template or new flow as an unsaved copy, a saved one as it is. */
  const edit = (f: Flow, unsaved: boolean) => {
    setRunId(null);
    setFlow(f);
    setDirty(unsaved);
    edited.current = false;
    setSelected(null);
    setNav('flow');
    fit();
  };
  const openEntry = async (e: FlowEntry) => {
    if (e.id !== flow.id && !mayLeave()) return;
    const savedFlow = lists.saved.find((f) => f.id === e.id);
    if (savedFlow) {
      if (e.id !== flow.id) edit(structuredClone(savedFlow), false);
      else (setRunId(null), setNav('flow'));
    } else if (e.last) {
      // Only its executions are left: open the flow as it was in the latest one.
      const r = await orch.getRun(e.last.id);
      if (r.ok) edit(structuredClone(r.data.flow), true);
      else say(r.error);
    }
  };
  const useTemplate = (f: Flow) => {
    if (!mayLeave()) return;
    edit(cloneFlow(f), true);
    setSideOpen(false);
  };
  const newFlow = () => {
    if (!mayLeave()) return;
    edit(emptyFlow(newFlowId(), 'New flow'), false);
    setSideOpen(false);
  };
  const duplicate = (e: FlowEntry) => {
    const f = lists.saved.find((x) => x.id === e.id);
    if (!f || !mayLeave()) return;
    edit({ ...cloneFlow(f), name: `${f.name} copy`.slice(0, 60) }, true);
    setSideOpen(false);
  };
  const toDesign = () => {
    setRunId(null);
    setSelected(null);
    setSideOpen(false);
  };
  const openRun = (id: string) => {
    setRunId(id);
    setSelected(null);
    setNav('flow');
    setSideOpen(false);
  };
  const select = (id: string | null) => {
    setSelected(id);
    setSelectedEdge(null);
    if (id && !wide) setInspOpen(true);
  };
  const selectEdge = (id: string) => {
    setSelected(null);
    setSelectedEdge(id);
    if (!wide) setInspOpen(true);
  };
  // A run that stops with questions for the user opens them, once, so the answer is one step away.
  const askedKey = run?.status.state === 'paused' && run.status.pause.kind === 'questions' && run.pending ? `${run.id}:${run.retriesUsed}` : null;
  const seenAsk = useRef<string | null>(null);
  useEffect(() => {
    if (askedKey && seenAsk.current !== askedKey) {
      seenAsk.current = askedKey;
      setAnswering(true);
    }
    if (!askedKey) setAnswering(false);
  }, [askedKey]);
  const save = () =>
    void act(() => orch.saveFlow(flow), () => {
      setDirty(false);
      edited.current = false;
      say('Flow saved');
      void refreshLists();
    });
  const removeFlow = (e: FlowEntry) => {
    if (!e.saved) {
      // Nothing saved is left of it but its executions.
      const ids = e.runs.filter((r) => r.status.state !== 'running').map((r) => r.id);
      if (ids.length && confirm(`Delete ${ids.length === 1 ? 'its execution' : `its ${ids.length} executions`}? There is no saved flow to keep.`)) dropRuns(ids);
      return;
    }
    const kept = e.runs.length ? ` Its ${e.runs.length === 1 ? 'execution stays' : `${e.runs.length} executions stay`} in your list, marked “Runs only”.` : '';
    if (!confirm(`Delete the flow “${e.name}”?${kept}`)) return;
    void act(() => orch.deleteFlow(e.id), () => {
      void refreshLists();
      if (e.id === flow.id) edit(emptyFlow(newFlowId(), 'New flow'), false), setNav('flows');
    });
  };
  const suggest = (goal: string, thenRun = false) =>
    void act(
      () => orch.suggest(goal),
      ({ notes, ...f }) => {
        edit(f, true);
        edited.current = true;
        setSuggesting(false);
        setSideOpen(false);
        // Design and run: the same words are the first task, so the run dialog opens with them, to be checked and started.
        setRunGoal(thenRun ? goal : '');
        setDialog(thenRun);
        // Which agents run on OpenRouter, and why, so a paid model is never a surprise.
        say([thenRun ? 'Here is a flow for it, and your text as the first task. Check the folder, then start.' : 'Here is a flow for it. Change anything, then run it with a goal of its own.', ...(notes ?? [])].join(' '));
      },
    );
  const designRole = async (description: string) => {
    setDesigning(true);
    setCustomError(null);
    const r = await orch.customRole(description);
    setDesigning(false);
    if (r.ok) setDraft({ label: r.data.label, capability: r.data.capability, output: r.data.output, instructions: r.data.instructions });
    else setCustomError(r.error);
  };
  const addCustom = (d: RoleDraft) => {
    const n = newCustomNode(flowRef.current, { label: d.label, capability: d.capability, output: d.output }, d.instructions, models, (() => {
      const c = document.querySelector('.mc-stage')?.getBoundingClientRect();
      const k = flowRef.current.nodes.length % 5;
      return { x: (c ? c.width / 2 : 300) - 116 + k * 26, y: (c ? c.height / 2 : 200) - 60 + k * 26 };
    })());
    editFlow((f) => ({ ...f, nodes: [...f.nodes, n] }));
    setSelected(n.id);
    setInspOpen(true);
    setCustomOpen(false);
    setDraft(null);
    say('Role added. Connect it to the other agents.');
  };
  const popOut = async (runIdNow: string, node: string) => {
    const r = await orch.popOut(runIdNow, node);
    if (r.ok) {
      say('Opening the live session…');
      onSession(r.data.session);
    } else say(r.error);
  };
  const copyText = (t: string) => void navigator.clipboard?.writeText(t).then(() => say('Copied'), () => say('Could not copy'));

  // Running a flow saves it first, so every execution has a flow to live under.
  const createRun = (goal: string, project: string, skipGrill: boolean, readLinks: boolean, docs: string[]) =>
    void act(
      async () => {
        if (dirty || !isSaved) {
          const saved = await orch.saveFlow(flow);
          if (!saved.ok) return { ok: false as const, error: saved.error };
        }
        return orch.createRun({ flow, goal, project, skipGrill, readLinks, docs });
      },
      (r) => {
        setDialog(false);
        setDirty(false);
        edited.current = false;
        setRun(r);
        setRunId(r.id);
        setSelected(null);
        setNav('flow');
        void refreshLists();
      },
    );

  /** Start a run like an earlier one, with the flow and brief it had. It starts at once, as everything it needs is the earlier run's. */
  const rerun = useCallback(
    async (id: string) => {
      setBusy(true);
      setError(null);
      const old = await orch.getRun(id);
      const made = old.ok ? await orch.createRun(rerunRequest(old.data)) : old;
      const started = made.ok ? await orch.start(made.data.id) : made;
      setBusy(false);
      if (!started.ok) {
        // Nothing opens, so the reason is shown here: a run that cannot start must never look like a button that does nothing.
        setRerunProblem({ id, message: started.error, project: old.ok ? old.data.project : '', git: /git repository/i.test(started.error) });
        return;
      }
      setRerunProblem(null);
      setRun(started.data);
      setRunId(started.data.id);
      setSelected(null);
      setRunView('timeline');
      void refreshLists();
      say('Running it again');
    },
    [refreshLists, say],
  );
  const fixAndRerun = async () => {
    if (!rerunProblem) return;
    setBusy(true);
    const r = await orch.projectInit(rerunProblem.project);
    setBusy(false);
    if (!r.ok) return setRerunProblem({ ...rerunProblem, message: r.error, git: false });
    void rerun(rerunProblem.id);
  };

  /** Put an older execution's design back in the editor, as the flow's current design. */
  // ------------------------------------------------------------------ deleting and quick actions
  const [logOpen, setLogOpen] = useState(() => {
    try {
      return localStorage.getItem('ttm.orch.log') === '1';
    } catch {
      return false;
    }
  });
  const toggleLog = () =>
    setLogOpen((o) => {
      try {
        localStorage.setItem('ttm.orch.log', o ? '0' : '1');
      } catch {
        /* the choice is simply not remembered */
      }
      return !o;
    });
  /** Show a run an action returned, then read it again: a run that finishes at once can announce it on the event
   *  stream before the reply arrives, and the reply must not put the older state back. */
  const applyRun = (r: Run) => {
    setRun(r);
    void orch.getRun(r.id).then((x) => x.ok && setRun((cur) => (cur && cur.id === x.data.id ? x.data : cur)));
  };

  // The plan limits apply to every flow and to runs that are going, so they are saved as they are typed.
  const saveTimer = useRef<number>(0);
  const changeLimits = (b: Budget) => {
    setLimitsState(b);
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(async () => {
      const r = await orch.setLimits(b);
      if (r.ok) say('Limits saved');
      else {
        say(r.error);
        const again = await orch.limits();
        if (again.ok) setLimitsState(again.data);
      }
    }, 500);
  };
  const openLimits = () => {
    setSelected(null);
    setInspOpen(true);
  };
  const [menu, setMenu] = useState<MenuState | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);

  /** Remove executions. The ones the open page shows go with them; any branches their builders made are kept. */
  const dropRuns = (ids: string[]) =>
    void act(
      async () => {
        for (const id of ids) {
          const r = await orch.deleteRun(id);
          if (!r.ok) return r;
        }
        return { ok: true as const, data: undefined };
      },
      () => {
        if (runId && ids.includes(runId)) setRunId(null);
        void refreshRuns();
      },
    ).then(() => void refreshRuns());
  const removeRun = (r: RunSummary) => {
    if (r.status.state === 'running') return say('Stop it first, then delete it.');
    if (confirm(`Delete the execution “${r.goal}”?`)) dropRuns([r.id]);
  };
  const clearFinished = (e: FlowEntry) => {
    const ids = e.runs.filter((r) => isFinished(r.status)).map((r) => r.id);
    if (ids.length && confirm(`Delete ${ids.length === 1 ? 'the finished execution' : `the ${ids.length} finished executions`} of “${e.name}”? Ones still going stay.`)) dropRuns(ids);
  };
  const flowMenu = (e: FlowEntry): MenuItem[] => {
    const items: MenuItem[] = [{ label: 'Open', icon: 'workflow', onSelect: () => void openEntry(e) }];
    if (e.saved) items.push({ label: 'Duplicate', icon: 'files', onSelect: () => duplicate(e) });
    if (e.runs.some((r) => isFinished(r.status))) items.push({ label: 'Delete finished executions', icon: 'minus', onSelect: () => clearFinished(e) });
    items.push({ kind: 'separator' }, e.saved ? { label: 'Delete flow', icon: 'trash', danger: true, onSelect: () => removeFlow(e) } : { label: 'Delete executions', icon: 'trash', danger: true, onSelect: () => removeFlow(e) });
    return items;
  };
  const runMenu = (r: RunSummary): MenuItem[] => {
    const items: MenuItem[] = [{ label: 'Open', icon: 'workflow', onSelect: () => openRun(r.id) }];
    if (r.id === runId) items.push({ label: 'Use this version', icon: 'retry', onSelect: () => useThisVersion() });
    if (r.status.state === 'running') items.push({ label: 'Stop', icon: 'stop', onSelect: () => void act(() => orch.cancel(r.id), (x) => (r.id === runId && applyRun(x), void refreshRuns())) });
    items.push({ kind: 'separator' }, { label: 'Delete execution', icon: 'trash', danger: true, disabled: r.status.state === 'running', onSelect: () => removeRun(r) });
    return items;
  };

  const useThisVersion = () => {
    if (!run) return;
    if (edited.current && !confirm('Replace your unsaved changes with this older version?')) return;
    setFlow(structuredClone(run.flow));
    setDirty(true);
    edited.current = false;
    setRunId(null);
    fit();
  };

  return (
    <>
      <header className="mc-top">
        <button type="button" className="mc-btn mc-exit" title="Leave the orchestrator and go back to single agent mode: your sessions" onClick={onExit}>
          <Icon name="agent" /> Single agent
        </button>
        {!wide && (
          <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Flows and executions" onClick={() => setSideOpen((o) => !o)}>
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
        <UsageMeters gauges={meters} api={apiMeters} />
        <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon mc-hide-narrow" aria-label="Settings and plan limits" title="Plan limits and flow settings" onClick={openLimits}>
          <Icon name="sliders" />
        </button>
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
              <button type="button" className="mc-btn mc-btn-primary" disabled={!!problem} onClick={() => (setRunGoal(''), setDialog(true))} title={problem ?? 'Start a run of this flow'}>
                <Icon name="play" /> Run
              </button>
            </>
          ) : (
            <>
              <button type="button" className="mc-btn mc-hide-narrow" onClick={useThisVersion} title="Put this execution's design back in the editor">
                <Icon name="retry" /> Use this version
              </button>
              <button type="button" className="mc-btn" aria-label="Back to design" title="Back to the design" onClick={toDesign}>
                <Icon name="edit" /> <span className="mc-hide-narrow">Back to design</span>
              </button>
            </>
          )}
          {!phone && !showTimeline && (
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
        <aside className="mc-side" data-open={sideOpen} aria-label="Flows and executions">
          {!loaded ? (
            <p className="mc-empty" role="status">
              Loading…
            </p>
          ) : (
          <Sidebar
            level={nav}
            entries={entries}
            starters={lists.starters}
            flowId={flow.id}
            flowName={flow.name}
            saved={isSaved}
            dirty={dirty}
            runId={runId}
            canRun={!problem}
            onOpenFlow={(e) => void openEntry(e)}
            onUseStarter={useTemplate}
            onNewFlow={newFlow}
            onSuggest={() => (setError(null), setSuggesting(true))}
            onDuplicate={duplicate}
            onDelete={removeFlow}
            onBack={() => setNav('flows')}
            onDesign={toDesign}
            onOpenRun={openRun}
            onNewRun={() => (setSideOpen(false), setRunGoal(''), setDialog(true))}
            onDeleteRun={removeRun}
            onRerun={(r) => rerun(r.id)}
            openMenu={setMenu}
            flowMenu={flowMenu}
            runMenu={runMenu}
          />
          )}
        </aside>

        <main className="mc-stage" data-list={showList || showTimeline}>
          {run && <RunBar run={run} busy={busy} link={link} onAnswer={() => setAnswering(true)} onMoreRounds={(n) => void act(() => orch.moreRounds(run.id, n), applyRun)} onApprove={(n) => void act(() => orch.approve(run.id, n), applyRun)} onResume={(waive) => void act(() => orch.resume(run.id, waive), applyRun)} onLimits={openLimits} onCancel={() => void act(() => orch.cancel(run.id), applyRun)} onCopy={copyText} view={runView} onView={setRunView} onRerun={() => rerun(run.id)} />}
          {showTimeline ? (
            <Timeline run={run!} selected={selected} />
          ) : showList ? (
            <NodeList flow={shown} run={run} selected={selected} onSelect={select} />
          ) : (
            <ReactFlowProvider>
              <FlowCanvas flow={shown} run={run} selected={selected} selectedEdge={selectedEdge} editable={editable} onFlow={editFlow} onSelect={select} onSelectEdge={selectEdge} onProblem={say} fitToken={fitToken} viewKey={run ? `run-${run.id}` : flow.id} roles={roles} onAddAt={addNode} onTidy={() => (editFlow(tidy), fit())} openMenu={setMenu} />
              {editable && <AddAgent roles={roles} count={flow.nodes.length} onAdd={addNode} onCustom={() => (setDraft(null), setCustomError(null), setCustomOpen(true))} />}
            </ReactFlowProvider>
          )}
          {editable && <Assistant flow={flow} onReplace={(f) => (editFlow(() => f), fit())} onDesign={() => (setError(null), setSuggesting(true))} />}
          {editable && flow.nodes.length === 0 && (
            <div className="mc-empty" style={{ position: 'absolute', inset: '32% 0 auto', zIndex: 3 }}>
              <p style={{ margin: '0 0 0.75rem' }}>An empty flow. Add agents from the bar below, open a starter, or</p>
              <button type="button" className="mc-btn" onClick={() => (setError(null), setSuggesting(true))}>
                <Icon name="wand" /> Design a flow
              </button>
            </div>
          )}
          {run && run.status.state !== 'grilling' && (
            <>
              <LogToggle open={logOpen} count={run.log?.length ?? 0} live={run.status.state === 'running'} onToggle={toggleLog} />
              {logOpen && <LogPanel run={run} onClose={toggleLog} />}
            </>
          )}
          {error && !dialog && run?.status.state !== 'grilling' && (
            <p className="mc-error" role="alert" style={{ position: 'absolute', left: '0.75rem', bottom: run ? '3.5rem' : '0.75rem', right: '0.75rem', margin: 0, zIndex: 6 }}>
              {error}
            </p>
          )}
          {toast && (
            <div className="mc-toast" role="status">
              {toast}
            </div>
          )}
          {customOpen && <CustomRoleDialog busy={designing} error={customError} draft={draft} onDesign={designRole} onAdd={addCustom} onClose={() => (setCustomOpen(false), setDraft(null), setCustomError(null))} />}
          {suggesting && <SuggestDialog busy={busy} error={error} onSuggest={suggest} onClose={() => (setSuggesting(false), setError(null))} />}
          {rerunProblem && <RerunProblemDialog message={rerunProblem.message} project={rerunProblem.project} git={rerunProblem.git} busy={busy} onFix={() => void fixAndRerun()} onClose={() => setRerunProblem(null)} />}
        {dialog && <NewRunDialog flow={flow} limits={limits} recents={recents} busy={busy} error={error} web={!!models.web} initialGoal={runGoal} onCreate={createRun} onClose={() => (setDialog(false), setRunGoal(''), setError(null))} />}
          {run?.status.state === 'grilling' && (
            <GrillDialog
              run={run}
              thinking={thinking}
              error={error}
              onAnswer={(a) => void doGrill(a)}
              onStart={(brief) => void act(() => orch.start(run.id, brief), applyRun)}
              onStop={() => void act(() => orch.cancel(run.id), applyRun)}
            />
          )}
        </main>

        <aside className="mc-inspector" data-open={inspOpen} aria-label="Details">
          <Inspector
            flow={shown}
            node={selectedNode}
            edge={selectedEdgeObj}
            run={run}
            models={models}
            editable={editable}
            goal={run?.goal ?? shown.name}
            limits={limits}
            onLimits={changeLimits}
            onNode={(id, patch) => editFlow((f) => ({ ...f, nodes: f.nodes.map((n) => (n.id === id ? { ...n, ...patch } : n)) }))}
            onEdge={(id, patch) => editFlow((f) => ({ ...f, edges: f.edges.map((e) => (e.id === id ? { ...e, ...patch } : e)) }))}
            onFlow={(patch) => editFlow((f) => ({ ...f, ...patch }))}
            onDelete={(id) => (editFlow((f) => ({ ...f, nodes: f.nodes.filter((n) => n.id !== id), edges: f.edges.filter((e) => e.from !== id && e.to !== id) })), setSelected(null))}
            onWritePrompt={async (n) => {
              const r = await orch.nodePrompt(n.role, n.custom, n.name, run?.goal ?? shown.name);
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
      {answering && run?.pending && (
        <QuestionsDialog run={run} busy={busy} error={error} onClose={() => setAnswering(false)} onAnswer={(a) => void act(() => orch.answers(run.id, a), (r) => (applyRun(r), setAnswering(false)))} />
      )}
      {menu && <ContextMenu menu={menu} onClose={closeMenu} />}
    </>
  );
}
