// The two dialogs: starting a run, and the questions the orchestrator asks first.

import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { RecentDir } from '../dirs';
import { orch } from './api';
import { Icon, Field, Switch } from './bits';
import { CAPABILITY_SHORT, tildePath, DESIGN_STEP_TEXT, OUTPUT_TEXT, answersFor, goalHint, waitingQuestions, hasLink, choose, consistent, hasChoices, startingAnswer, writes, writeInstead, type Answer, type DesignNote, type DesignStep } from './model';
import type { Budget, Capability, Flow, Output, Question, RepoState, Run } from './types';

const PROJECT_KEY = 'ttm.orch.project';

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Dialog({ title, onClose, children, foot }: { title: string; onClose?: () => void; children: ReactNode; foot: ReactNode }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!onClose) return;
    const key = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [onClose]);
  // Keyboard focus belongs to the dialog while it is open, and goes back to where it was when it closes.
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    const el = box.current;
    if (el && !el.contains(document.activeElement)) (el.querySelector<HTMLElement>(FOCUSABLE) ?? el).focus();
    return () => before?.focus?.();
  }, []);
  const trap = (e: React.KeyboardEvent) => {
    if (e.key !== 'Tab' || !box.current) return;
    const items = [...box.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
    if (!items.length) return;
    const [first, last] = [items[0]!, items[items.length - 1]!];
    if (e.shiftKey && document.activeElement === first) (e.preventDefault(), last.focus());
    else if (!e.shiftKey && document.activeElement === last) (e.preventDefault(), first.focus());
  };
  return (
    <div className="mc-scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose?.()}>
      <div ref={box} className="mc-dialog" role="dialog" aria-modal="true" aria-label={title} tabIndex={-1} onKeyDown={trap}>
        <div className="mc-panel-head">
          <span style={{ flex: 1 }}>{title}</span>
          {onClose && (
            <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Close" onClick={onClose}>
              <Icon name="close" />
            </button>
          )}
        </div>
        <div className="mc-dialog-body">{children}</div>
        <div className="mc-dialog-foot">{foot}</div>
      </div>
    </div>
  );
}

function remembered(): string {
  try {
    return localStorage.getItem(PROJECT_KEY) ?? '';
  } catch {
    return '';
  }
}

export function NewRunDialog({ flow, limits, recents, busy, error, web, initialGoal, locked, onCreate, onClose }: {
  flow: Flow;
  /** There is a web account, so the links in the goal can be read for the agents. */
  web: boolean;
  /** A goal to start with, such as the text a flow was just designed from. */
  initialGoal?: string;
  /** The flow is kept in a folder linked to a directory, so its runs use that one. */
  locked?: { folder: string; dir: string } | null;
  limits: Budget;
  recents: RecentDir[];
  busy: boolean;
  error: string | null;
  onCreate: (goal: string, project: string, skipGrill: boolean, readLinks: boolean, docs: string[]) => void;
  onClose: () => void;
}) {
  const [goal, setGoal] = useState(initialGoal ?? '');
  const [readLinks, setReadLinks] = useState(true);
  const [docs, setDocs] = useState('');
  const [project, setProject] = useState(locked?.dir ?? (remembered() || recents[0]?.path || ''));
  const [skip, setSkip] = useState(false);
  const goalRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => goalRef.current?.focus(), []);

  // Agents that change files work on copies made from the folder's latest commit, so the folder must be a git
  // repository with one. Say so as soon as a folder is chosen, and offer to set it up.
  const needsGit = flow.nodes.some(writes);
  const [repo, setRepo] = useState<RepoState | 'checking'>('checking');
  const [settingUp, setSettingUp] = useState(false);
  const [repoError, setRepoError] = useState<string | null>(null);
  useEffect(() => {
    const path = project.trim();
    setRepoError(null);
    if (!path) return setRepo('missing');
    setRepo('checking');
    let stale = false;
    const id = window.setTimeout(async () => {
      const r = await orch.projectState(path);
      if (!stale) setRepo(r.ok ? r.data.state : 'missing');
    }, 350);
    return () => {
      stale = true;
      window.clearTimeout(id);
    };
  }, [project]);
  const setUpGit = async () => {
    setSettingUp(true);
    setRepoError(null);
    const r = await orch.projectInit(project.trim());
    setSettingUp(false);
    if (r.ok) setRepo(r.data.state);
    else setRepoError(r.error);
  };
  const ready = goal.trim() !== '' && project.trim() !== '' && (!needsGit || repo === 'ready');
  const go = () => {
    if (!ready || busy) return;
    try {
      // A fixed directory is not a choice, so it is not what the next run starts from.
      if (!locked) localStorage.setItem(PROJECT_KEY, project.trim());
    } catch {
      /* a private window: the choice is simply not remembered */
    }
    onCreate(goal.trim(), project.trim(), skip, readLinks, web && needsGit ? docs.split('\n').map((d) => d.trim()).filter(Boolean) : []);
  };
  const b = limits;
  return (
    <Dialog
      title="New run"
      onClose={onClose}
      foot={
        <>
          <button type="button" className="mc-btn" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="mc-btn mc-btn-primary" disabled={!ready || busy} onClick={go}>
            <Icon name="play" /> {busy ? 'Starting…' : skip ? 'Review the brief' : 'Start with questions'}
          </button>
        </>
      }
    >
      <Field label="What should the team do this time?" hint={goalHint(flow)}>
        <textarea
          ref={goalRef}
          className="mc-field"
          rows={5}
          value={goal}
          maxLength={4000}
          placeholder="e.g. Add a dark mode toggle to settings and remember the choice"
          onChange={(e) => setGoal(e.target.value)}
          onKeyDown={(e) => (e.metaKey || e.ctrlKey) && e.key === 'Enter' && go()}
        />
      </Field>
      <Field
        label="Runs in"
        hint={locked ? `Fixed: this flow is kept in “${locked.folder}”, a folder for this directory, so it runs here. To run it somewhere else, copy it to another folder.` : needsGit ? 'Builders work in their own copy of this folder, so it needs to be a git repository with a commit.' : 'The folder the agents work in.'}
      >
        {locked ? (
          <div className="mc-locked mc-mono" aria-label="Project folder, fixed">
            <Icon name="folder" /> {tildePath(locked.dir)}
          </div>
        ) : (
          <input className="mc-field mc-mono" value={project} placeholder="/path/to/project" onChange={(e) => setProject(e.target.value)} />
        )}
        {!locked && recents.length > 0 && (
          <div className="mc-chips" style={{ marginTop: '0.5rem' }}>
            {recents.slice(0, 6).map((r) => (
              <button key={r.path} type="button" className="mc-chip" aria-pressed={project === r.path} onClick={() => setProject(r.path)} title={r.path}>
                {r.name}
              </button>
            ))}
          </div>
        )}
        <div className="mc-repo" data-state={repo} role="status">
          {repo === 'ready' && <span><Icon name="check" /> Git repository, ready.</span>}
          {repo === 'checking' && <span className="mc-hint">Checking the folder…</span>}
          {repo === 'missing' && <span className="mc-hint">{project.trim() ? 'That folder was not found.' : 'Choose a folder.'}</span>}
          {(repo === 'notARepo' || repo === 'noCommits') && (
            <>
              <span>
                <Icon name="alert" /> {repo === 'notARepo' ? 'This folder is not a git repository.' : 'This repository has no commits yet.'}{' '}
                {needsGit ? 'Builders need one to work on a copy.' : 'This flow changes no files, so you can go on without.'}
              </span>
              <button type="button" className="mc-btn mc-btn-sm" disabled={settingUp} onClick={() => void setUpGit()}>
                {settingUp ? 'Setting up…' : 'Set up git here'}
              </button>
              <span className="mc-hint">{repo === 'notARepo' ? 'Runs git init, then commits' : 'Commits'} what is in the folder now as the starting point, leaving anything your ignore rules exclude.</span>
            </>
          )}
          {repoError && <span className="mc-error">{repoError}</span>}
        </div>
      </Field>
      <Switch on={skip} onChange={setSkip} label="Skip the questions" hint="Use what you typed as the brief." />
      {web && needsGit && (
        <Field label="Documentation to read" hint="Optional. One address per line. Up to three sites are crawled (about ten pages each, a credit a page) into the builder’s folder, under docs/_ref, for it to read. They are never committed.">
          <textarea className="mc-field mc-mono" rows={2} placeholder="https://docs.rs/serde/latest/serde/" value={docs} onChange={(e) => setDocs(e.target.value)} />
        </Field>
      )}
      {web && hasLink(goal) && (
        <Switch on={readLinks} onChange={setReadLinks} label="Read the links in it" hint="Fetches up to three pages through your web accounts (about a credit each) and gives their text to every agent." />
      )}
      <p className="mc-hint">
        Stops at {b.stopWindowPct}% of any plan window, or when the run has used {b.claudePct}% of Claude’s or {b.codexPct}% of Codex’s 5-hour window. Change them any time in Settings.
      </p>
      {error && <p className="mc-error">{error}</p>}
    </Dialog>
  );
}

/** One question: choices to tick, with "Something else" to write an answer, or a text box if it has no choices. */
function Ask({ q, answer, onChange }: { q: Question; answer: Answer; onChange: (a: Answer) => void }) {
  if (!hasChoices(q)) {
    return (
      <div className="mc-q">
        <p className="mc-q-text">{q.text}</p>
        <textarea className="mc-field" rows={2} value={answer.text} placeholder={q.recommended || 'Your answer'} onChange={(e) => onChange({ ...answer, text: e.target.value })} />
      </div>
    );
  }
  const kind = q.multiple ? 'checkbox' : 'radio';
  return (
    <div className="mc-q" role={q.multiple ? 'group' : 'radiogroup'} aria-label={q.text}>
      <p className="mc-q-text">{q.text}</p>
      {q.multiple && <p className="mc-hint" style={{ margin: '-0.25rem 0 0.4rem' }}>Choose any that apply.</p>}
      <div className="mc-opts">
        {q.options!.map((o) => {
          const on = answer.chosen.includes(o) && (q.multiple || !answer.other);
          return (
            <button key={o} type="button" role={kind} aria-checked={on} className="mc-opt" onClick={() => onChange(choose(q, answer, o))}>
              <span className="mc-opt-mark" data-kind={kind} aria-hidden="true">{on && <Icon name="check" />}</span>
              <span className="mc-opt-text">{o}</span>
              {o === q.recommended && <span className="mc-opt-tag">Suggested</span>}
            </button>
          );
        })}
        <button type="button" role={kind} aria-checked={answer.other} className="mc-opt" onClick={() => onChange(answer.other ? { ...answer, other: false } : writeInstead(q, answer))}>
          <span className="mc-opt-mark" data-kind={kind} aria-hidden="true">{answer.other && <Icon name="check" />}</span>
          <span className="mc-opt-text">Something else…</span>
        </button>
      </div>
      {answer.other && (
        <textarea
          className="mc-field mc-opt-write"
          rows={2}
          autoFocus
          value={answer.text}
          placeholder="Your own answer"
          aria-label={`Your own answer to: ${q.text}`}
          onChange={(e) => onChange({ ...answer, text: e.target.value })}
        />
      )}
    </div>
  );
}

/** What the user ends up with from the custom-role dialog. */
export interface RoleDraft {
  label: string;
  capability: Capability;
  output: Output;
  instructions: string;
}

export function CustomRoleDialog({ busy, error, draft, onDesign, onAdd, onClose }: {
  busy: boolean;
  error: string | null;
  /** Set once Sonnet has designed the role; the user can then change anything before adding it. */
  draft: RoleDraft | null;
  onDesign: (description: string) => void;
  onAdd: (d: RoleDraft) => void;
  onClose: () => void;
}) {
  const [description, setDescription] = useState('');
  const [d, setD] = useState<RoleDraft | null>(draft);
  useEffect(() => setD(draft), [draft]);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  const set = (patch: Partial<RoleDraft>) => setD((cur) => {
    if (!cur) return cur;
    const next = { ...cur, ...patch };
    return { ...next, output: consistent(next.capability, next.output) };
  });

  if (!d) {
    return (
      <Dialog
        title="Custom role"
        onClose={busy ? undefined : onClose}
        foot={
          <>
            <button type="button" className="mc-btn" disabled={busy} onClick={onClose}>
              Cancel
            </button>
            <button type="button" className="mc-btn mc-btn-primary" disabled={busy || description.trim().length < 8} onClick={() => onDesign(description.trim())}>
              <Icon name="wand" /> {busy ? 'Designing…' : 'Design with Sonnet'}
            </button>
          </>
        }
      >
        <Field label="What should this agent do?" hint="Describe the job in your own words. Sonnet picks what it may do, what it hands back, and writes its instructions. You can change all of it before it is added.">
          <textarea
            ref={ref}
            className="mc-field"
            rows={5}
            value={description}
            maxLength={2000}
            disabled={busy}
            placeholder="e.g. Check the change for security problems such as injection and unsafe input handling, and reject it if it finds any"
            onChange={(e) => setDescription(e.target.value)}
          />
        </Field>
        {busy && (
          <div className="mc-thinking" role="status">
            <span className="mc-spinner" /> Designing the role…
          </div>
        )}
        {error && <p className="mc-error">{error}</p>}
      </Dialog>
    );
  }

  const ok = d.label.trim() !== '' && d.instructions.trim().length >= 40;
  return (
    <Dialog
      title="Check the role"
      onClose={onClose}
      foot={
        <>
          <button type="button" className="mc-btn" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="mc-btn mc-btn-primary" disabled={!ok} onClick={() => onAdd({ ...d, label: d.label.trim(), instructions: d.instructions.trim() })}>
            <Icon name="plus" /> Add to the flow
          </button>
        </>
      }
    >
      <Field label="Name">
        <input className="mc-field" value={d.label} maxLength={40} onChange={(e) => set({ label: e.target.value })} />
      </Field>
      <div className="mc-grid2">
        <Field label="May">
          <select className="mc-field" value={d.capability} onChange={(e) => set({ capability: e.target.value as Capability })}>
            {(Object.keys(CAPABILITY_SHORT) as Capability[]).map((c) => (
              <option key={c} value={c}>
                {CAPABILITY_SHORT[c]}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Hands back">
          <select className="mc-field" value={d.output} disabled={d.capability === 'write'} onChange={(e) => set({ output: e.target.value as Output })}>
            {(Object.keys(OUTPUT_TEXT) as Output[])
              .filter((o) => (d.capability === 'write' ? o === 'change' : o !== 'change'))
              .map((o) => (
                <option key={o} value={o}>
                  {OUTPUT_TEXT[o]}
                </option>
              ))}
          </select>
        </Field>
      </div>
      <p className="mc-hint" style={{ margin: '0 0 1rem' }}>
        {d.capability === 'write' ? 'It will ask you before it starts, and work in its own copy of the project.' : d.output === 'verdict' ? 'It can send work back to an earlier agent.' : 'It cannot change any file.'}
      </p>
      <Field label="Instructions" hint="Sent the same way on every turn, so the prompt cache holds. The goal is given separately.">
        <textarea className="mc-field" rows={8} value={d.instructions} maxLength={2000} onChange={(e) => set({ instructions: e.target.value })} />
      </Field>
    </Dialog>
  );
}

export function SuggestDialog({ busy, step, error, onSuggest, onClose }: { busy: boolean; step: DesignStep | null; error: string | null; onSuggest: (goal: string, thenRun: boolean) => void; onClose: () => void }) {
  const [goal, setGoal] = useState('');
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  const ready = !busy && !!goal.trim();
  return (
    <Dialog
      title="Design a flow"
      onClose={busy ? undefined : onClose}
      foot={
        <>
          <button type="button" className="mc-btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="mc-btn" disabled={!ready} onClick={() => onSuggest(goal.trim(), true)} title="Design the flow, then say what to do with it this time, starting from this text">
            <Icon name="play" /> Design and run
          </button>
          <button type="button" className="mc-btn mc-btn-primary" disabled={!ready} onClick={() => onSuggest(goal.trim(), false)}>
            <Icon name="wand" /> {busy ? 'Designing…' : 'Design'}
          </button>
        </>
      }
    >
      <Field
        label="What kind of work is this flow for?"
        hint="Sonnet designs a reusable team: the agents, their models and who hands work to whom. It opens in the editor and nothing runs. Each time you run it you say what to do, so describe the kind of job, not one task. To use this text as the first task too, choose Design and run."
      >
        <textarea
          ref={ref}
          className="mc-field"
          rows={5}
          value={goal}
          maxLength={4000}
          disabled={busy}
          placeholder="e.g. Find the cause of a bug, fix it, and have the fix reviewed"
          onChange={(e) => setGoal(e.target.value)}
        />
      </Field>
      {busy && (
        <div className="mc-thinking" role="status">
          <span className="mc-spinner" /> {step ? DESIGN_STEP_TEXT[step] : 'Designing a team for it'}. Each step can take up to a minute.
        </div>
      )}
      {error && <p className="mc-error">{error}</p>}
    </Dialog>
  );
}

/** How a flow was made, once it has been: what each of the three steps found or changed. */
export function DesignNotesDialog({ notes, onClose }: { notes: DesignNote[]; onClose: () => void }) {
  return (
    <Dialog
      title="How this flow was designed"
      onClose={onClose}
      foot={
        <button type="button" className="mc-btn mc-btn-primary" onClick={onClose}>
          Open the flow
        </button>
      }
    >
      <p className="mc-hint">It is in the editor now and nothing has run. Change anything you like.</p>
      <ul className="mc-notes">
        {notes.map((n, i) => (
          <li key={i} data-failed={n.failed || undefined}>
            <span className="mc-pill" data-tone={n.failed ? 'warn' : 'idle'}>
              {n.step}
            </span>{' '}
            {n.text}
          </li>
        ))}
      </ul>
    </Dialog>
  );
}

export function GrillDialog({ run, thinking, error, onAnswer, onStart, onStop }: {
  run: Run;
  thinking: boolean;
  error: string | null;
  onAnswer: (answers: string[]) => void;
  onStart: (brief: string) => void;
  onStop: () => void;
}) {
  const g = run.grill;
  const [picks, setPicks] = useState<Record<number, Answer>>({});
  const [brief, setBrief] = useState(g?.brief ?? '');
  // A new round (or a new brief) starts with a clean form.
  useEffect(() => setPicks({}), [g?.round]);
  useEffect(() => setBrief(g?.brief ?? ''), [g?.brief]);
  const pending = g?.pending ?? [];

  // Waiting on the orchestrator: just a small box with a spinner, no header, until there is something to answer.
  if (thinking || (!pending.length && !g?.brief && !error)) {
    return (
      <div className="mc-scrim">
        <div className="mc-thinking-box" role="status" aria-live="polite">
          <span className="mc-spinner" />
          <span className="mc-thinking-text">The orchestrator is thinking…</span>
          <button type="button" className="mc-btn mc-btn-quiet mc-btn-sm" onClick={onStop}>
            Stop
          </button>
        </div>
      </div>
    );
  }

  let title = 'The orchestrator has some questions';
  let body: ReactNode;
  let foot: ReactNode;

  if (pending.length) {
    body = (
      <>
        <p className="mc-hint" style={{ margin: '0 0 1rem' }}>
          Round {g!.round} of 3. The suggested answer is ticked: change what you like, or just continue.
        </p>
        {pending.map((q, i) => (
          <Ask key={`${g!.round}-${i}`} q={q} answer={picks[i] ?? startingAnswer(q)} onChange={(a) => setPicks((cur) => ({ ...cur, [i]: a }))} />
        ))}
      </>
    );
    foot = (
      <>
        <button type="button" className="mc-btn" onClick={onStop}>
          Stop
        </button>
        <button type="button" className="mc-btn mc-btn-primary" onClick={() => onAnswer(answersFor(pending, picks))}>
          Continue
        </button>
      </>
    );
  } else if (g?.brief) {
    title = 'Check the brief';
    body = (
      <>
        <p className="mc-hint" style={{ marginTop: 0 }}>
          This is what every agent is told to do. Change anything that is wrong, then start.
        </p>
        <textarea className="mc-field" rows={14} value={brief} onChange={(e) => setBrief(e.target.value)} />
      </>
    );
    foot = (
      <>
        <button type="button" className="mc-btn" onClick={onStop}>
          Stop
        </button>
        <button type="button" className="mc-btn mc-btn-primary" disabled={!brief.trim()} onClick={() => onStart(brief)}>
          <Icon name="play" /> Start the run
        </button>
      </>
    );
  } else {
    // The call failed and nothing is waiting: offer another go.
    title = 'That did not work';
    body = <p className="mc-error" style={{ marginTop: 0 }}>{error}</p>;
    foot = (
      <>
        <button type="button" className="mc-btn" onClick={onStop}>
          Stop
        </button>
        <button type="button" className="mc-btn mc-btn-primary" onClick={() => onAnswer([])}>
          <Icon name="retry" /> Try again
        </button>
      </>
    );
  }

  return (
    <Dialog title={title} foot={foot}>
      {body}
      {error && (pending.length > 0 || g?.brief) && <p className="mc-error">{error}</p>}
    </Dialog>
  );
}

/** The questions an agent asked that only the user can answer: one box each, and the answers go back with the work. */
export function QuestionsDialog({ run, busy, error, onAnswer, onClose }: {
  run: Run;
  busy: boolean;
  error: string | null;
  onAnswer: (answers: string[]) => void;
  onClose: () => void;
}) {
  const pending = run.pending;
  const questions = useMemo(() => (pending ? waitingQuestions(pending) : []), [pending]);
  const [picks, setPicks] = useState<Record<number, Answer>>({});
  useEffect(() => setPicks({}), [pending?.questions.join('\n')]);
  if (!pending) return null;
  const who = run.flow.nodes.find((n) => n.id === pending.node)?.name ?? 'An agent';
  const to = run.flow.nodes.find((n) => n.id === pending.target)?.name ?? 'the agent it goes back to';
  const before = (run.asked?.[pending.node] ?? []).filter((a) => a.answer !== null);
  return (
    <Dialog
      title={`${who} has questions`}
      onClose={onClose}
      foot={
        <>
          <button type="button" className="mc-btn" onClick={onClose}>
            Later
          </button>
          <button type="button" className="mc-btn mc-btn-primary" disabled={busy} onClick={() => onAnswer(answersFor(questions, picks))}>
            <Icon name="play" /> {busy ? 'Sending…' : `Send to ${to}`}
          </button>
        </>
      }
    >
      <p className="mc-hint" style={{ margin: '0 0 1rem' }}>
        Your answers go to {to}, which revises its work with them; {who} then looks again. The suggested answer is picked for each; change any you disagree with.
      </p>
      {questions.map((q, i) => (
        <Ask key={`${i}-${q.text}`} q={q} answer={picks[i] ?? startingAnswer(q)} onChange={(a) => setPicks((cur) => ({ ...cur, [i]: a }))} />
      ))}
      {before.length > 0 && (
        <details className="mc-hint">
          <summary>Asked and answered earlier ({before.length})</summary>
          <ul>
            {before.map((a, i) => (
              <li key={i}>
                {a.question} → {a.answer || 'no answer'}
              </li>
            ))}
          </ul>
        </details>
      )}
      {error && <p className="mc-error">{error}</p>}
    </Dialog>
  );
}

/** A run could not be started again. Says why, and for a project that has no git history to work from, offers to set one up. */
export function RerunProblemDialog({ message, project, git, busy, onFix, onClose }: {
  message: string;
  project: string;
  /** The reason is that the project has no repository or no commit to build from. */
  git: boolean;
  busy: boolean;
  onFix: () => void;
  onClose: () => void;
}) {
  return (
    <Dialog
      title="It could not be run again"
      onClose={onClose}
      foot={
        <>
          <button type="button" className="mc-btn" onClick={onClose}>
            {git ? 'Not now' : 'Close'}
          </button>
          {git && (
            <button type="button" className="mc-btn mc-btn-primary" disabled={busy} onClick={onFix}>
              <Icon name="branch" /> {busy ? 'Setting up…' : 'Set up git and run again'}
            </button>
          )}
        </>
      }
    >
      <p>{message}</p>
      {git && (
        <p className="mc-hint">
          This does <code>git init</code> if needed, then makes a first commit of what is in <span className="mc-mono">{project}</span>, leaving out anything its ignore rules exclude. Your files are not changed.
        </p>
      )}
    </Dialog>
  );
}
