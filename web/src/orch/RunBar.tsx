// The strip over the canvas while a run is shown: where it is, what it has used, and what it needs from you.

import { Icon, StatusPill } from './bits';
import { builderBranches, cacheHit, fmtTokens, pauseInfo, runTokens, totalTokens } from './model';
import type { Run } from './types';

interface Props {
  run: Run;
  busy: boolean;
  link: boolean;
  onApprove: (node: string) => void;
  onResume: () => void;
  onCancel: () => void;
  onCopy: (text: string) => void;
}

export function RunBar({ run, busy, link, onApprove, onResume, onCancel, onCopy }: Props) {
  const info = pauseInfo(run);
  const tokens = runTokens(run);
  const hit = cacheHit(tokens);
  const live = run.status.state === 'running' || run.status.state === 'paused' || run.status.state === 'grilling';
  const done = run.status.state === 'done';
  const branches = builderBranches(run);
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
          {hit !== null && (
            <span>
              <b>{Math.round(hit * 100)}%</b> cached
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
                onClick={() => (info.primary!.action === 'approve' ? onApprove(info.primary!.node!) : onResume())}
              >
                <Icon name={info.primary.action === 'approve' ? 'check' : 'play'} /> {info.primary.label}
              </button>
            )}
            <button type="button" className="mc-btn" disabled={busy} onClick={onCancel}>
              Stop the run
            </button>
          </div>
        </div>
      )}

      {done && branches.length > 0 && (
        <div className="mc-notice" data-tone="ok">
          <div className="mc-notice-title">Done. The changes are on their own branch.</div>
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
