// The run's log, in a drawer along the bottom of the chart: what each agent was sent, what it answered, and what the
// orchestrator decided because of it, with the skills and sub-agents agents used along the way.

import { useEffect, useMemo, useRef, useState } from 'react';
import type { IconName } from '../icons';
import { Icon } from './bits';
import { clock, liveLines, logView, skillCount, type LogFilter } from './model';
import type { LogEntry, LogKind, Run } from './types';

const ICON: Record<LogKind, IconName> = {
  decision: 'route',
  request: 'arrow-up',
  response: 'arrow-down',
  skill: 'skill',
  subagent: 'agent',
  event: 'dot',
  error: 'alert',
};

const LABEL: Record<LogKind, string> = {
  decision: 'Decision',
  request: 'Sent',
  response: 'Answer',
  skill: 'Skill',
  subagent: 'Sub-agent',
  event: 'Event',
  error: 'Error',
};

export function LogToggle({ open, count, live, onToggle }: { open: boolean; count: number; live: boolean; onToggle: () => void }) {
  return (
    <button type="button" className="mc-logtab" aria-expanded={open} aria-controls="mc-log" data-open={open} onClick={onToggle}>
      <Icon name="todo" /> Logs
      {count > 0 && <span className="mc-logtab-n">{count}</span>}
      {live && <span className="mc-logtab-live" aria-label="running" />}
      <span style={{ display: 'inline-flex', transform: open ? 'rotate(90deg)' : 'rotate(-90deg)' }}>
        <Icon name="chevron" />
      </span>
    </button>
  );
}

function Entry({ e, name, open, onToggle }: { e: LogEntry; name: string | null; open: boolean; onToggle: () => void }) {
  const long = !!e.body;
  const lines = e.body ? e.body.split('\n').length : 0;
  // A decision's reason is short and is the point of the log, so it is shown. A prompt or an answer is long, so it opens on request.
  const inline = e.kind === 'decision' || e.kind === 'error';
  return (
    <li className="mc-log-row" data-kind={e.kind}>
      <time className="mc-log-time" dateTime={new Date(e.at * 1000).toISOString()}>{clock(e.at)}</time>
      <span className="mc-log-badge" title={LABEL[e.kind]}>
        <Icon name={ICON[e.kind]} />
      </span>
      <div className="mc-log-main">
        <div className="mc-log-title">
          {name && !e.title.includes(name) && <span className="mc-log-node">{name}</span>}
          <span>{e.title}</span>
          {long && !inline && (
            <button type="button" className="mc-log-more" aria-expanded={open} onClick={onToggle}>
              {open ? 'Hide' : e.kind === 'request' ? 'Show what it was sent' : 'Show the answer'} · {lines} {lines === 1 ? 'line' : 'lines'}
            </button>
          )}
        </div>
        {long && (inline || open) && <pre className="mc-log-body">{e.body}</pre>}
      </div>
    </li>
  );
}

export function LogPanel({ run, onClose }: { run: Run; onClose: () => void }) {
  const log = run.log ?? [];
  const [filter, setFilter] = useState<LogFilter>('all');
  const [opened, setOpened] = useState<Set<number>>(new Set());
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const names = useMemo(() => Object.fromEntries(run.flow.nodes.map((n) => [n.id, n.name])), [run.flow.nodes]);
  const shown = useMemo(() => logView(log, filter), [log, filter]);
  const live = useMemo(() => liveLines(run).filter((l) => filter === 'all' || filter === `node:${l.node}`), [run, filter]);
  const skills = skillCount(log);
  // The chosen agent's own working lines (the tools it called, what it said), which are not in the run's log.
  const activity = filter.startsWith('node:') ? (run.nodes[filter.slice(5)]?.transcript ?? []) : [];

  // Keep following the newest line while the reader is at the bottom; leave them alone once they scroll up.
  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [shown.length, live, activity.length, filter]);
  useEffect(() => {
    const key = (e: KeyboardEvent) => e.key === 'Escape' && !document.querySelector('.mc-scrim, .mc-menu') && onClose();
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [onClose]);

  const chip = (id: LogFilter, label: string, extra?: number) => (
    <button key={id} type="button" className="mc-chip" aria-pressed={filter === id} onClick={() => setFilter(id)}>
      {label}
      {extra ? <span className="mc-chip-n">{extra}</span> : null}
    </button>
  );

  return (
    <section id="mc-log" className="mc-runlog" aria-label="Run log">
      <div className="mc-log-head">
        <div className="mc-chips" role="group" aria-label="Show">
          {chip('all', 'Everything', log.length)}
          {chip('skills', 'Skills & sub-agents', skills)}
          {run.flow.nodes.map((n) => chip(`node:${n.id}`, n.name))}
        </div>
        <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Close the log" onClick={onClose}>
          <Icon name="close" />
        </button>
      </div>
      <div
        className="mc-log-scroll"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        {shown.length === 0 && live.length === 0 && activity.length === 0 ? (
          <p className="mc-empty">
            {filter === 'skills'
              ? 'No agent has used a skill or started a sub-agent in this run. Agents here run without skills, and only an agent that changes files can start sub-agents.'
              : 'Nothing yet. Each step the flow takes will be written here.'}
          </p>
        ) : (
          <ol className="mc-log-list">
            {shown.map((e, i) => {
              const key = log.indexOf(e);
              return <Entry key={`${key}-${i}`} e={e} name={e.node ? (names[e.node] ?? e.node) : null} open={opened.has(key)} onToggle={() => setOpened((s) => (s.has(key) ? new Set([...s].filter((k) => k !== key)) : new Set(s).add(key)))} />;
            })}
            {live.map((l) => (
              <li key={`live-${l.node}`} className="mc-log-row mc-log-live">
                <span className="mc-log-time" />
                <span className="mc-log-badge"><span className="mc-spinner" /></span>
                <div className="mc-log-main">
                  <div className="mc-log-title">
                    <span className="mc-log-node">{names[l.node] ?? l.node}</span>
                    <span className="mc-log-now">{l.line}</span>
                  </div>
                </div>
              </li>
            ))}
          </ol>
        )}
        {activity.length > 0 && (
          <div className="mc-log-activity">
            <p className="mc-label">What {names[filter.slice(5)] ?? 'this agent'} did, step by step</p>
            <pre className="mc-log-body">{activity.join('\n')}</pre>
          </div>
        )}
      </div>
    </section>
  );
}
