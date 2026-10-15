// The two dialogs: starting a run, and the questions the orchestrator asks first.

import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { RecentDir } from '../dirs';
import { Icon, Field, Switch } from './bits';
import { CAPABILITY_SHORT, OUTPUT_TEXT, answersFor, choose, consistent, hasChoices, startingAnswer, writeInstead, type Answer } from './model';
import type { Capability, Flow, Output, Question, Run } from './types';

const PROJECT_KEY = 'ttm.orch.project';

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function Dialog({ title, onClose, children, foot }: { title: string; onClose?: () => void; children: ReactNode; foot: ReactNode }) {
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
  const [picks, setPicks] = useState<Record<number, Answer>>({});
  const [brief, setBrief] = useState(g?.brief ?? '');
  // A new round (or a new brief) starts with a clean form.
  useEffect(() => setPicks({}), [g?.round]);
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
