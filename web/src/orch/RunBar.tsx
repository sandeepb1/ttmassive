// The strip over the canvas while a run is shown: where it is, what it has used, and what it needs from you.

import { useEffect, useState } from 'react';
import { orch } from './api';
import { Icon, StatusPill } from './bits';
import { builderBranches, cacheBreakCount, cacheHit, fmtDuration, fmtTokens, pauseInfo, runCost, runCredits, runTokens, summary, totalTokens, usdLabel, writes } from './model';
import type { RepoState, Run } from './types';

interface Props {
  run: Run;
  busy: boolean;
  link: boolean;
  onApprove: (node: string) => void;
  onResume: (waive?: boolean) => void;
  onLimits: () => void;
  onCancel: () => void;
  onCopy: (text: string) => void;
}

/**
 * Builders work on copies made from the project's latest commit. When the project cannot give one, say so before a
 * builder hits it (or after, if it already has) and offer to set it up, then carry on.
 */
function GitNotice({ run, busy, onFixed }: { run: Run; busy: boolean; onFixed: (wasFailed: boolean) => void }) {
  const pending = run.flow.nodes.some((n) => writes(n) && run.nodes[n.id]?.status !== 'done');
  const active = run.status.state === 'running' || run.status.state === 'paused';
  const [state, setState] = useState<RepoState | null>(null);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const check = run.project && pending && active;
  // Look again when the run changes state: a builder failing is when the answer matters most.
  const key = `${run.id}:${run.status.state}:${run.status.state === 'paused' ? run.status.pause.kind : ''}`;
  useEffect(() => {
    if (!check) return setState(null);
    let stale = false;
    void orch.projectState(run.project).then((r) => !stale && setState(r.ok ? r.data.state : null));
    return () => {
      stale = true;
    };
  }, [key, check, run.project]);
  if (!check || !state || state === 'ready') return null;

  const failedOnIt = run.status.state === 'paused' && run.status.pause.kind === 'failed' && /git repository/i.test(run.status.pause.message);
  const fix = async () => {
    setWorking(true);
    setError(null);
    const r = await orch.projectInit(run.project);
    setWorking(false);
    if (!r.ok) return setError(r.error);
    setState(r.data.state);
    if (r.data.state === 'ready') onFixed(failedOnIt);
  };
  return (
    <div className="mc-notice" data-tone={failedOnIt ? 'bad' : 'warn'} role="alert">
      <div className="mc-notice-title">{state === 'missing' ? 'The project folder is gone' : state === 'noCommits' ? 'The project has no commits yet' : 'The project is not a git repository'}</div>
      <p className="mc-notice-detail">
        {state === 'missing'
          ? `${run.project} was not found, so a builder has nowhere to work.`
          : 'A builder works on its own copy of the project, made from its latest commit, so it needs one.'}
      </p>
      {state !== 'missing' && (
        <>
          <div className="mc-notice-actions">
            <button type="button" className="mc-btn mc-btn-primary" disabled={busy || working} onClick={() => void fix()}>
              <Icon name="branch" /> {working ? 'Setting up…' : failedOnIt ? 'Set up git and try again' : 'Set up git here'}
            </button>
          </div>
          <p className="mc-notice-detail mc-mono">
            {state === 'notARepo' ? 'git init, then a first commit of' : 'A first commit of'} what is in {run.project}, leaving out anything its ignore rules exclude.
          </p>
        </>
      )}
      {error && <p className="mc-error">{error}</p>}
    </div>
  );
}

export function RunBar({ run, busy, link, onApprove, onResume, onLimits, onCancel, onCopy }: Props) {
  const info = pauseInfo(run);
  const tokens = runTokens(run);
  const hit = cacheHit(tokens);
  const live = run.status.state === 'running' || run.status.state === 'paused' || run.status.state === 'grilling';
  const done = run.status.state === 'done';
  const branches = builderBranches(run);
  const breaks = cacheBreakCount(run);
  const cost = runCost(run);
  const credits = runCredits(run);
  const sum = summary(run);
  const failed = run.status.state === 'paused' && (run.status.pause.kind === 'failed' || run.status.pause.kind === 'retriesExhausted');

  return (
    <div className="mc-runbar">
      <div className="mc-runbar-main">
        <StatusPill status={run.status} />
        <span className="mc-runbar-goal" title={run.goal}>
          {run.goal}
        </span>
        <span className="mc-runbar-stats" title="Everything the agents read and wrote, cache included">
          <span>
            <b>{fmtTokens(totalTokens(tokens))}</b> tok
          </span>
          {cost > 0 && (
            <span title="What OpenRouter has charged for this run's agents on it">
              <b>{usdLabel(cost)}</b> spent
            </span>
          )}
          {credits > 0 && (
            <span title="Firecrawl credits the agents' web tools have used in this run">
              <b>{credits}</b> web {credits === 1 ? 'credit' : 'credits'}
            </span>
          )}
          {hit !== null && (
            <span>
              <b>{Math.round(hit * 100)}%</b> cached
            </span>
          )}
          {breaks > 0 && (
            <span style={{ color: 'var(--mc-bad)' }} title="Turns that got far less from the prompt cache than the one before">
              <b style={{ color: 'inherit' }}>{breaks}</b> cache {breaks === 1 ? 'break' : 'breaks'}
            </span>
          )}
          {run.waived && (
            <span title="You chose to carry on past the plan limits">
              <b>limits</b> ignored
            </span>
          )}
          {run.retriesUsed > 0 && (
            <span>
              <b>{run.retriesUsed}</b> sent back
            </span>
          )}
        </span>
        {!link && <span className="mc-pill" data-tone="warn">offline</span>}
        {live && (
          <button type="button" className="mc-btn mc-btn-sm mc-btn-danger" disabled={busy} onClick={onCancel}>
            <Icon name="stop" /> Stop
          </button>
        )}
      </div>

      <GitNotice run={run} busy={busy} onFixed={(wasFailed) => wasFailed && onResume(false)} />

      {info && (
        <div className="mc-notice" data-tone={failed ? 'bad' : undefined} role="alert">
          <div className="mc-notice-title">{info.title}</div>
          <p className="mc-notice-detail">{info.detail}</p>
          <div className="mc-notice-actions">
            {info.primary && (
              <button
                type="button"
                className="mc-btn mc-btn-primary"
                disabled={busy}
                title={info.primary.waive ? 'Ignore the plan limits for the rest of this run' : undefined}
                onClick={() => (info.primary!.action === 'approve' ? onApprove(info.primary!.node!) : onResume(info.primary!.waive))}
              >
                <Icon name={info.primary.action === 'approve' ? 'check' : 'play'} /> {info.primary.label}
              </button>
            )}
            {info.limits && (
              <button type="button" className="mc-btn" onClick={onLimits}>
                <Icon name="sliders" /> Change the limits
              </button>
            )}
            <button type="button" className="mc-btn" disabled={busy} onClick={onCancel}>
              Stop the run
            </button>
          </div>
        </div>
      )}

      {done && (
        <div className="mc-notice" data-tone="ok">
          <div className="mc-notice-title">{sum.duration !== null ? `Done in ${fmtDuration(sum.duration)}` : 'Done'}</div>
          <p className="mc-notice-detail mc-mono">
            {[
              `${fmtTokens(sum.tokens)} tokens`,
              runCost(run) > 0 ? `${usdLabel(runCost(run))} on OpenRouter` : null,
              credits > 0 ? `${credits} web credit${credits === 1 ? '' : 's'}` : null,
              sum.cached !== null ? `${Math.round(sum.cached * 100)}% from the cache` : null,
              sum.sentBack ? `sent back ${sum.sentBack}×` : null,
              sum.cheaper ? `${sum.cheaper} on a cheaper model` : null,
              sum.cacheBreaks ? `${sum.cacheBreaks} cache ${sum.cacheBreaks === 1 ? 'break' : 'breaks'}` : null,
            ]
              .filter(Boolean)
              .join(' · ')}
          </p>
          {branches.length > 0 && <p className="mc-notice-detail">The changes are on their own branch.</p>}
          {branches.map((b) => {
            const cmd = `git -C ${run.project} diff HEAD...${b.branch}`;
            return (
              <div key={b.node}>
                <p className="mc-notice-detail">
                  {b.name}: <span className="mc-mono">{b.branch}</span>
                </p>
                <div className="mc-notice-actions">
                  <button type="button" className="mc-btn mc-btn-sm" onClick={() => onCopy(cmd)}>
                    <Icon name="files" /> Copy the diff command
                  </button>
                  <button type="button" className="mc-btn mc-btn-sm" onClick={() => onCopy(`git -C ${run.project} merge ${b.branch}`)}>
                    <Icon name="branch" /> Copy the merge command
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
