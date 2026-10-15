// A small chat widget in the design view: say how the flow should change, and Claude or Codex changes it. Each change
// is applied at once and can be undone, so asking is never a commitment.

import { useEffect, useRef, useState } from 'react';
import { orch } from './api';
import { Icon } from './bits';
import type { Flow } from './types';

type Engine = 'claude' | 'codex';
const ENGINE_KEY = 'ttm.orch.assistant';

interface Turn {
  request: string;
  /** What it did, in its own words; or why it could not. */
  reply: string;
  failed?: boolean;
  /** The flow before this change, for undo, and after it, to tell whether the person has edited since. */
  before?: Flow;
  after?: string;
  undone?: boolean;
}

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

function savedEngine(): Engine {
  try {
    return localStorage.getItem(ENGINE_KEY) === 'codex' ? 'codex' : 'claude';
  } catch {
    return 'claude';
  }
}

const EXAMPLES_FULL = ['Add a security reviewer after the builder', 'Use cheaper models where that is safe', 'Run the research in parallel with the design'];
const EXAMPLES_EMPTY = ['Fix a bug and have the fix reviewed', 'Research a topic and write it up', 'Build a feature with a plan, a builder and a judge'];

export function Assistant({ flow, onReplace }: { flow: Flow; onReplace: (f: Flow) => void }) {
  const [open, setOpen] = useState(false);
  const [engine, setEngine] = useState<Engine>(savedEngine);
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [turns, setTurns] = useState<Turn[]>([]);
  const flowRef = useRef(flow);
  flowRef.current = flow;
  const input = useRef<HTMLTextAreaElement>(null);
  const log = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) input.current?.focus();
  }, [open]);
  useEffect(() => {
    log.current?.scrollTo({ top: log.current.scrollHeight });
  }, [turns, busy, open]);

  const pick = (e: Engine) => {
    setEngine(e);
    try {
      localStorage.setItem(ENGINE_KEY, e);
    } catch {
      /* the choice is simply not remembered */
    }
  };

  const send = async (request = text) => {
    const ask = request.trim();
    if (!ask || busy) return;
    const before = structuredClone(flowRef.current);
    setText('');
    setBusy(true);
    const r = await orch.editFlow(before, ask, engine);
    setBusy(false);
    if (!r.ok) return setTurns((t) => [...t, { request: ask, reply: r.error, failed: true }]);
    // Someone edited the chart while it was working: putting its answer over that would lose their change.
    if (!same(flowRef.current, before)) {
      return setTurns((t) => [...t, { request: ask, reply: 'The flow changed while I was working, so I left it alone. Ask again.', failed: true }]);
    }
    const unchanged = same(r.data.flow, before);
    if (!unchanged) onReplace(r.data.flow);
    setTurns((t) => [...t, { request: ask, reply: r.data.summary, before: unchanged ? undefined : before, after: JSON.stringify(r.data.flow) }]);
  };

  const undo = (i: number) => {
    const turn = turns[i];
    if (!turn?.before) return;
    // Put back what it was before that change, unless the person has made changes of their own since.
    if (!same(flowRef.current, JSON.parse(turn.after ?? 'null')) && !confirm('You have changed the flow since. Undoing this will also remove those changes. Go on?')) return;
    onReplace(turn.before);
    setTurns((t) => t.map((x, j) => (j === i ? { ...x, undone: true } : x)));
  };
  const lastApplied = turns.reduce((n, t, i) => (t.before && !t.undone ? i : n), -1);

  if (!open) {
    return (
      <button type="button" className="mc-bot-fab" onClick={() => setOpen(true)} aria-label="Ask the assistant to change this flow" title="Ask the assistant to change this flow">
        <Icon name="wand" /> <span className="mc-bot-fab-text">Ask AI</span>
      </button>
    );
  }
  const examples = flow.nodes.length ? EXAMPLES_FULL : EXAMPLES_EMPTY;
  return (
    <section className="mc-bot" aria-label="Design assistant" onKeyDown={(e) => e.key === 'Escape' && setOpen(false)}>
      <div className="mc-bot-head">
        <span className="mc-bot-title">
          <Icon name="wand" /> Design assistant
        </span>
        <div className="mc-seg" role="radiogroup" aria-label="Which AI to ask">
          {(['claude', 'codex'] as const).map((e) => (
            <button key={e} type="button" role="radio" aria-checked={engine === e} data-engine={e} onClick={() => pick(e)}>
              {e === 'claude' ? 'Claude' : 'Codex'}
            </button>
          ))}
        </div>
        <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Close the assistant" onClick={() => setOpen(false)}>
          <Icon name="close" />
        </button>
      </div>

      <div className="mc-bot-log" ref={log} aria-live="polite">
        {turns.length === 0 && !busy && (
          <div className="mc-bot-empty">
            <p>{flow.nodes.length ? 'Tell me how to change this flow. I will add, remove or change agents and rewire them, and you can undo it.' : 'Describe the work and I will build the flow for it.'}</p>
            <div className="mc-chips">
              {examples.map((x) => (
                <button key={x} type="button" className="mc-chip" onClick={() => void send(x)}>
                  {x}
                </button>
              ))}
            </div>
          </div>
        )}
        {turns.map((t, i) => (
          <div key={i} className="mc-bot-turn">
            <p className="mc-bot-you">{t.request}</p>
            <p className="mc-bot-reply" data-failed={t.failed || undefined}>
              {t.reply}
            </p>
            {t.before && (
              <button type="button" className="mc-btn mc-btn-sm" disabled={t.undone || i !== lastApplied} onClick={() => undo(i)} title={i !== lastApplied && !t.undone ? 'Undo the later changes first' : undefined}>
                <Icon name="retry" /> {t.undone ? 'Undone' : 'Undo'}
              </button>
            )}
          </div>
        ))}
        {busy && (
          <div className="mc-bot-turn">
            <p className="mc-bot-reply mc-thinking" role="status">
              <span className="mc-spinner" /> {engine === 'claude' ? 'Claude' : 'Codex'} is changing the flow…
            </p>
          </div>
        )}
      </div>

      <form
        className="mc-bot-form"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <textarea
          ref={input}
          className="mc-field"
          rows={2}
          value={text}
          maxLength={2000}
          disabled={busy}
          placeholder="e.g. add a security reviewer after the builder"
          aria-label="What should change?"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button type="submit" className="mc-btn mc-btn-primary mc-btn-icon" disabled={busy || !text.trim()} aria-label="Send">
          <Icon name="arrow-up" />
        </button>
      </form>
    </section>
  );
}
