// The two dialogs: starting a run, and the questions the orchestrator asks first.

import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { RecentDir } from '../dirs';
import { Icon, Field, Switch } from './bits';
import { answersFor } from './model';
import type { Flow, Run } from './types';

const PROJECT_KEY = 'ttm.orch.project';

function Dialog({ title, onClose, children, foot }: { title: string; onClose?: () => void; children: ReactNode; foot: ReactNode }) {
  useEffect(() => {
    if (!onClose) return;
    const key = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [onClose]);
  return (
    <div className="mc-scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose?.()}>
      <div className="mc-dialog" role="dialog" aria-modal="true" aria-label={title}>
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

export function NewRunDialog({ flow, recents, busy, error, onCreate, onClose }: {
  flow: Flow;
  recents: RecentDir[];
  busy: boolean;
  error: string | null;
  onCreate: (goal: string, project: string, skipGrill: boolean) => void;
  onClose: () => void;
}) {
  const [goal, setGoal] = useState('');
  const [project, setProject] = useState(remembered() || recents[0]?.path || '');
  const [skip, setSkip] = useState(false);
  const goalRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => goalRef.current?.focus(), []);
  const ready = goal.trim() !== '' && project.trim() !== '';
  const go = () => {
    if (!ready || busy) return;
    try {
      localStorage.setItem(PROJECT_KEY, project.trim());
    } catch {
      /* a private window: the choice is simply not remembered */
    }
    onCreate(goal.trim(), project.trim(), skip);
  };
  const b = flow.budget;
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
      <Field label="What should the team do?" hint="Say it the way you would to a colleague. The orchestrator will ask what it needs to know before it starts.">
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
      <Field label="Project folder" hint="A git repository with at least one commit. Builders work in their own copy of it.">
        <input className="mc-field mc-mono" value={project} placeholder="/path/to/project" onChange={(e) => setProject(e.target.value)} />
        {recents.length > 0 && (
          <div className="mc-chips" style={{ marginTop: '0.5rem' }}>
            {recents.slice(0, 6).map((r) => (
              <button key={r.path} type="button" className="mc-chip" aria-pressed={project === r.path} onClick={() => setProject(r.path)} title={r.path}>
                {r.name}
              </button>
            ))}
          </div>
        )}
      </Field>
      <Switch on={skip} onChange={setSkip} label="Skip the questions" hint="Use what you typed as the brief." />
      <p className="mc-hint">
        Stops at {b.stopWindowPct}% of any plan window, or when the run has used {b.claudePct}% of Claude’s or {b.codexPct}% of Codex’s 5-hour window. Change these in the flow settings.
      </p>
      {error && <p className="mc-error">{error}</p>}
    </Dialog>
  );
}

export function SuggestDialog({ busy, error, onSuggest, onClose }: { busy: boolean; error: string | null; onSuggest: (goal: string) => void; onClose: () => void }) {
  const [goal, setGoal] = useState('');
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  return (
    <Dialog
      title="Suggest a flow"
      onClose={busy ? undefined : onClose}
      foot={
        <>
          <button type="button" className="mc-btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="mc-btn mc-btn-primary" disabled={busy || !goal.trim()} onClick={() => onSuggest(goal.trim())}>
            <Icon name="wand" /> {busy ? 'Designing…' : 'Suggest'}
          </button>
        </>
      }
    >
      <Field label="What is the work?" hint="Sonnet picks the agents, their models and who hands work to whom. It opens in the editor for you to change; nothing runs.">
        <textarea
          ref={ref}
          className="mc-field"
          rows={5}
          value={goal}
          maxLength={4000}
          disabled={busy}
          placeholder="e.g. Find out why the nightly export is slow, fix it, and have it reviewed"
          onChange={(e) => setGoal(e.target.value)}
        />
      </Field>
      {busy && (
        <div className="mc-thinking" role="status">
          <span className="mc-spinner" /> Designing a team for it. This takes up to half a minute.
        </div>
      )}
      {error && <p className="mc-error">{error}</p>}
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
  const [typed, setTyped] = useState<Record<number, string>>({});
  const [brief, setBrief] = useState(g?.brief ?? '');
  // A new round (or a new brief) starts with a clean form.
  useEffect(() => setTyped({}), [g?.round]);
  useEffect(() => setBrief(g?.brief ?? ''), [g?.brief]);
  const pending = g?.pending ?? [];

  let title = 'The orchestrator has some questions';
  let body: ReactNode;
  let foot: ReactNode;

  if (thinking || (!pending.length && !g?.brief && !error)) {
    title = 'The orchestrator is thinking';
    body = (
      <div className="mc-thinking" role="status">
        <span className="mc-spinner" /> Reading your request{g && g.round > 0 ? ' and your answers' : ''}…
      </div>
    );
    foot = (
      <button type="button" className="mc-btn" onClick={onStop}>
        Stop
      </button>
    );
  } else if (pending.length) {
    body = (
      <>
        <p className="mc-hint" style={{ margin: '0 0 1rem' }}>
          Round {g!.round} of 3. Leave an answer empty to take the suggested one.
        </p>
        {pending.map((q, i) => (
          <div key={`${g!.round}-${i}`} className="mc-q">
            <p className="mc-q-text">{q.text}</p>
            <textarea className="mc-field" rows={2} value={typed[i] ?? ''} placeholder={q.recommended || 'Your answer'} onChange={(e) => setTyped({ ...typed, [i]: e.target.value })} />
            {q.recommended && (
              <button type="button" className="mc-btn mc-btn-sm mc-btn-quiet mc-q-use" onClick={() => setTyped({ ...typed, [i]: q.recommended })}>
                <Icon name="check" /> Use the suggestion
              </button>
            )}
          </div>
        ))}
      </>
    );
    foot = (
      <>
        <button type="button" className="mc-btn" onClick={onStop}>
          Stop
        </button>
        <button type="button" className="mc-btn mc-btn-primary" onClick={() => onAnswer(answersFor(pending, typed))}>
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
