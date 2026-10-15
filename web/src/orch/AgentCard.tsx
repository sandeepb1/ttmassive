// One agent, as a card. Used on the canvas (with connection handles) and in the phone list (without).

import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { Icon, ROLE_ICON } from './bits';
import { ROLE_INFO, cacheHit, fmtTokens, modelShown, totalTokens } from './model';
import type { FlowNode, NodeRun } from './types';

export interface CardData extends Record<string, unknown> {
  node: FlowNode;
  run?: NodeRun;
  /** The run is paused waiting for this agent to be approved. */
  waiting: boolean;
  selected: boolean;
}

function State({ run, waiting }: { run?: NodeRun; waiting: boolean }) {
  if (run?.status === 'running') return <span className="mc-spinner" role="status" aria-label="Running" />;
  if (run?.status === 'done') return <Icon name="check" />;
  if (run?.status === 'failed' || run?.status === 'interrupted') return <Icon name="alert" />;
  if (waiting) return <Icon name="pause" />;
  return null;
}

export function CardBody({ node, run, waiting }: Pick<CardData, 'node' | 'run' | 'waiting'>) {
  const t = run?.tokens;
  const total = t ? totalTokens(t) : 0;
  const hit = t ? cacheHit(t) : null;
  const pct = (n: number) => `${total ? (n / total) * 100 : 0}%`;
  const last = run?.status === 'running' ? run.transcript.at(-1) : undefined;
  const model = modelShown(node, run);
  const breaks = run?.cacheBreaks ?? 0;
  return (
    <>
      <div className="mc-card-head">
        <span className="mc-role">
          <Icon name={ROLE_ICON[node.role]} />
        </span>
        <div style={{ minWidth: 0 }}>
          <div className="mc-card-name">{node.name}</div>
          <div className="mc-card-role">{ROLE_INFO[node.role].label}</div>
        </div>
        <span className="mc-card-state">
          <State run={run} waiting={waiting} />
        </span>
      </div>
      <div className="mc-card-meta">
        <span className="mc-model" title={model.cheaper ? 'Switched to a cheaper model because your plan is filling up' : undefined}>
          {model.label}
        </span>
        {model.cheaper && <span className="mc-flag mc-flag-cool">cheaper</span>}
        {node.checkpoint && (
          <span className="mc-flag" title="Waits for your approval before it starts">
            <Icon name="pause" /> approve
          </span>
        )}
        {breaks > 0 && (
          <span className="mc-flag mc-flag-bad" title="A turn got far less from the prompt cache than the one before it, so the prompt was paid for again">
            <Icon name="alert" /> cache ×{breaks}
          </span>
        )}
        {run && run.attempts > 1 && (
          <span className="mc-flag" title="Sent back by the judge">
            <Icon name="retry" /> ×{run.attempts}
          </span>
        )}
      </div>
      {t && total > 0 && (
        <div className="mc-usage" title="Fresh input and cache writes, output, cache reads">
          <div className="mc-usage-line">
            <span>{fmtTokens(total)} tok</span>
            <span>{hit === null ? '' : `${Math.round(hit * 100)}% cached`}</span>
          </div>
          <div className="mc-usage-bar">
            <i style={{ width: pct(t.input + t.cacheWrite) }} />
            <i style={{ width: pct(t.output) }} />
            <i style={{ width: pct(t.cacheRead) }} />
          </div>
        </div>
      )}
      {last && <div className="mc-last">{last}</div>}
      {run?.error && <div className="mc-last" style={{ color: 'var(--mc-bad)' }}>{run.error}</div>}
    </>
  );
}

function AgentCardImpl({ data }: NodeProps) {
  const { node, run, waiting, selected } = data as CardData;
  return (
    <div className="mc-card" data-engine={node.engine} data-status={run?.status} data-waiting={waiting || undefined} data-selected={selected}>
      <Handle id="in" type="target" position={Position.Left} />
      <CardBody node={node} run={run} waiting={waiting} />
      <Handle id="out" type="source" position={Position.Right} />
      {/* A judge sends work back along the bottom; the builder it goes to takes it there. */}
      {node.role === 'judge' && <Handle id="back-out" type="source" position={Position.Bottom} title="Drag to a builder to send work back" />}
      {node.role === 'builder' && <Handle id="back-in" type="target" position={Position.Bottom} />}
    </div>
  );
}

export const AgentCard = memo(AgentCardImpl);
