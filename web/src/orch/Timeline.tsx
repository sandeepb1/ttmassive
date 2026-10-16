// A run as a timeline: every turn every agent took, in the order they finished, grouped into rounds (a round starts when
// work was sent back), each one opening to the full record of what the agent was asked, said, ran and handed on.

import { useEffect, useMemo, useRef, useState } from 'react';
import { orch } from './api';
import { Icon } from './bits';
import { rounds, turnClock, turnFacts, turnTone, turnsOf, type Round } from './model';
import type { Run, TraceEntry, Turn, TurnDetail } from './types';

const KIND_LABEL: Record<TraceEntry['kind'], string> = { prompt: 'Asked', say: 'Said', tool: 'Ran', result: 'Got back', note: 'Note' };

/** A long text shows its first lines and the rest on request, so a turn with a big file in it stays readable. */
function Clip({ text }: { text: string }) {
  const lines = text.split('\n');
  const [all, setAll] = useState(false);
  const long = lines.length > 14 || text.length > 1600;
  const shown = all || !long ? text : `${lines.slice(0, 14).join('\n').slice(0, 1600)}…`;
  return (
    <>
      <pre className="mc-tl-pre">{shown}</pre>
      {long && (
        <button type="button" className="mc-log-more" onClick={() => setAll(!all)}>
          {all ? 'Show less' : `Show all · ${lines.length} lines`}
        </button>
      )}
    </>
  );
}

/** What one turn did, fetched when it is opened (the page only holds a summary of each turn). */
function Detail({ runId, turn, live }: { runId: string; turn: Turn; live: boolean }) {
  const [d, setD] = useState<TurnDetail | 'missing' | 'loading'>('loading');
  useEffect(() => {
    let stale = false;
    setD('loading');
    void orch.turn(runId, turn.n).then((r) => !stale && setD(r.ok ? r.data : 'missing'));
    return () => {
      stale = true;
    };
  }, [runId, turn.n, live ? turn.ended : 0]);
  if (d === 'loading') return <p className="mc-hint">Loading…</p>;
  if (d === 'missing') return <p className="mc-hint">The full record of this turn was not kept.</p>;
  return (
    <div className="mc-tl-detail">
      {d.trace.length === 0 && !d.output && !d.error && <p className="mc-hint">Nothing was recorded for this turn.</p>}
      {d.trace.map((e, i) => (
        <div key={i} className="mc-tl-entry" data-kind={e.kind}>
          <div className="mc-tl-entry-head">
            <span className="mc-tl-kind">{KIND_LABEL[e.kind]}</span>
            {e.title && e.title !== 'prompt' && <span className="mc-tl-entry-title">{e.title}</span>}
          </div>
          {e.body && <Clip text={e.body} />}
        </div>
      ))}
      {d.error && (
        <div className="mc-tl-entry" data-kind="error">
          <div className="mc-tl-entry-head">
            <span className="mc-tl-kind">Failed</span>
          </div>
          <Clip text={d.error} />
        </div>
      )}
      {d.output != null && (
        <div className="mc-tl-entry" data-kind="output">
          <div className="mc-tl-entry-head">
            <span className="mc-tl-kind">Handed on</span>
          </div>
          <Clip text={JSON.stringify(d.output, null, 2)} />
        </div>
      )}
    </div>
  );
}

function TurnCard({ run, t, name, open, onToggle }: { run: Run; t: Turn; name: string; open: boolean; onToggle: () => void }) {
  const tone = turnTone(t);
  return (
    <li className="mc-tl-turn" data-tone={tone}>
      <span className="mc-tl-dot" aria-hidden="true" />
      <div className="mc-tl-card">
        <button type="button" className="mc-tl-head" aria-expanded={open} onClick={onToggle}>
          <time className="mc-tl-time">{turnClock(t.started)}</time>
          <span className="mc-tl-name">{name}</span>
          <span className="mc-tl-facts">{turnFacts(t, name).join(' · ')}</span>
          <span className="mc-tl-chev" data-open={open}>
            <Icon name="chevron" />
          </span>
        </button>
        {t.failed && <p className="mc-tl-fail">{t.failed}</p>}
        {t.decision && <p className="mc-tl-decision">{t.decision}</p>}
        {t.commit && (
          <p className="mc-tl-commit">
            Saved in commit <code>{t.commit}</code> on <code>{`ttm-orch/${run.id}-${t.node}`}</code>
          </p>
        )}
        {t.reason && <p className="mc-tl-reason">Why this model: {t.reason}</p>}
        {open && <Detail runId={run.id} turn={t} live={run.status.state === 'running'} />}
      </div>
    </li>
  );
}

function RoundHeader({ r, index, total }: { r: Round; index: number; total: number }) {
  return (
    <li className="mc-tl-round" aria-label={index === 0 ? 'First pass' : `Round ${index + 1}`}>
      <span className="mc-tl-round-mark" aria-hidden="true">
        {index === 0 ? <Icon name="play" /> : <Icon name="retry" />}
      </span>
      <span className="mc-tl-round-title">{index === 0 ? 'First pass' : `Round ${index + 1}`}</span>
      <span className="mc-tl-round-n">
        {r.turns.length} {r.turns.length === 1 ? 'turn' : 'turns'}
        {total > 1 && index === total - 1 ? ' · latest' : ''}
      </span>
    </li>
  );
}

export function Timeline({ run, selected }: { run: Run; selected: string | null }) {
  const [only, setOnly] = useState<string | null>(null);
  const [opened, setOpened] = useState<Set<number>>(new Set());
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const names = useMemo(() => Object.fromEntries(run.flow.nodes.map((n) => [n.id, n.name])), [run.flow.nodes]);
  const turns = run.turns ?? [];
  // Choosing an agent on the chart narrows the timeline to it, until another chip is chosen.
  const agent = only ?? null;
  const shown = useMemo(() => rounds(turnsOf(turns, agent)), [turns, agent]);
  const working = run.flow.nodes.filter((n) => run.nodes[n.id]?.status === 'running');
  const toggle = (n: number) => setOpened((s) => (s.has(n) ? new Set([...s].filter((x) => x !== n)) : new Set(s).add(n)));

  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [turns.length, working.length]);
  useEffect(() => {
    if (selected) setOnly(selected);
  }, [selected]);

  return (
    <section className="mc-timeline" aria-label="Timeline of the run">
      <div className="mc-tl-chips mc-chips" role="group" aria-label="Show">
        <button type="button" className="mc-chip" aria-pressed={agent === null} onClick={() => setOnly(null)}>
          Everything<span className="mc-chip-n">{turns.length}</span>
        </button>
        {run.flow.nodes.map((n) => (
          <button key={n.id} type="button" className="mc-chip" aria-pressed={agent === n.id} onClick={() => setOnly(agent === n.id ? null : n.id)}>
            {n.name}
          </button>
        ))}
      </div>
      <div
        className="mc-tl-scroll"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {turns.length === 0 && working.length === 0 ? (
          <p className="mc-empty">{run.log && run.log.length > 0 ? 'This run was made before turns were recorded. Its log is in the drawer below the chart.' : 'Nothing has run yet. Each turn an agent takes will appear here as it finishes.'}</p>
        ) : (
          <ol className="mc-tl-list">
            {shown.map((r, i) => (
              <li key={r.round} className="mc-tl-roundbox">
                <ol className="mc-tl-list">
                  <RoundHeader r={r} index={i} total={shown.length} />
                  {r.turns.map((t) => (
                    <TurnCard key={t.n} run={run} t={t} name={names[t.node] ?? t.node} open={opened.has(t.n)} onToggle={() => toggle(t.n)} />
                  ))}
                </ol>
              </li>
            ))}
            {working
              .filter((n) => agent === null || agent === n.id)
              .map((n) => (
                <li key={`now-${n.id}`} className="mc-tl-turn mc-tl-now" data-tone="live">
                  <span className="mc-tl-dot" aria-hidden="true" />
                  <div className="mc-tl-card">
                    <div className="mc-tl-head">
                      <span className="mc-spinner" />
                      <span className="mc-tl-name">{n.name}</span>
                      <span className="mc-tl-facts">working · {run.nodes[n.id]?.transcript.at(-1) ?? 'starting'}</span>
                    </div>
                  </div>
                </li>
              ))}
          </ol>
        )}
      </div>
    </section>
  );
}
