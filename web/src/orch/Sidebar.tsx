// The sidebar is a small tree. At the top it lists flows. Inside a flow it lists that flow's executions, newest first,
// with Design (the editor) above them. One list at a time, so a flow and its runs are never side by side as if they were
// the same kind of thing.

import { useState } from 'react';
import { Icon, StatusPill, ago } from './bits';
import { fmtTokens, runsLabel, whenLabel, type FlowEntry } from './model';
import type { Flow, RunSummary } from './types';

interface Props {
  level: 'flows' | 'flow';
  entries: FlowEntry[];
  starters: Flow[];
  /** The flow that is open, and the execution being viewed, if any. */
  flowId: string;
  flowName: string;
  /** The open flow is in the saved list. */
  saved: boolean;
  dirty: boolean;
  runId: string | null;
  canRun: boolean;
  onOpenFlow: (e: FlowEntry) => void;
  onUseStarter: (f: Flow) => void;
  onNewFlow: () => void;
  onSuggest: () => void;
  onDuplicate: (e: FlowEntry) => void;
  onDelete: (e: FlowEntry) => void;
  onBack: () => void;
  onDesign: () => void;
  onOpenRun: (id: string) => void;
  onNewRun: () => void;
}

export function Sidebar(p: Props) {
  return p.level === 'flows' ? <FlowList {...p} /> : <FlowPanel {...p} />;
}

function FlowList(p: Props) {
  const [showTemplates, setShowTemplates] = useState(p.entries.length === 0);
  return (
    <>
      <div className="mc-panel-head">
        <span style={{ flex: 1 }}>Flows</span>
      </div>
      <div className="mc-scroll">
        <div className="mc-grid2" style={{ marginBottom: '0.75rem' }}>
          <button type="button" className="mc-btn" onClick={p.onNewFlow}>
            <Icon name="plus" /> New flow
          </button>
          <button type="button" className="mc-btn" onClick={p.onSuggest} title="Describe the work and let Sonnet design the team">
            <Icon name="wand" /> Suggest
          </button>
        </div>

        {p.entries.length === 0 ? (
          <p className="mc-empty">No flows yet. Start from a template below, or build one.</p>
        ) : (
          <ul className="mc-list">
            {p.entries.map((e) => (
              <li key={e.id} className="mc-li">
                <button type="button" className="mc-row-btn" aria-current={e.id === p.flowId} onClick={() => p.onOpenFlow(e)}>
                  <span className="mc-row-title">{e.name}</span>
                  <span className="mc-row-sub">
                    <span>
                      {e.saved ? `${e.agents} agents` : 'Runs only'} · {runsLabel(e.runs.length)}
                    </span>
                  </span>
                  {e.last && (
                    <span className="mc-row-sub">
                      <span>{ago(e.last.created)}</span>
                      <StatusPill status={e.last.status} />
                    </span>
                  )}
                </button>
                {e.saved ? (
                  <span style={{ display: 'flex' }}>
                    <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Duplicate ${e.name}`} title="Make a copy to edit" onClick={() => p.onDuplicate(e)}>
                      <Icon name="files" />
                    </button>
                    <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Delete ${e.name}`} onClick={() => p.onDelete(e)}>
                      <Icon name="trash" />
                    </button>
                  </span>
                ) : (
                  <span />
                )}
              </li>
            ))}
          </ul>
        )}

        <button type="button" className="mc-disclose" aria-expanded={showTemplates} onClick={() => setShowTemplates((o) => !o)}>
          <span style={{ display: 'inline-flex', transform: showTemplates ? 'rotate(90deg)' : undefined }}>
            <Icon name="chevron" />
          </span>
          Templates
        </button>
        {showTemplates && (
          <ul className="mc-list">
            {p.starters.map((f) => (
              <li key={f.id}>
                <button type="button" className="mc-row-btn" onClick={() => p.onUseStarter(f)}>
                  <span className="mc-row-title">{f.name}</span>
                  <span className="mc-row-sub">{f.nodes.length} agents · opens as a copy for you to edit</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}

function FlowPanel(p: Props) {
  const mine: RunSummary[] = p.entries.find((e) => e.id === p.flowId)?.runs ?? [];
  const designing = p.runId === null;
  return (
    <>
      <div className="mc-panel-head">
        <button type="button" className="mc-btn mc-btn-quiet mc-btn-sm" onClick={p.onBack}>
          <Icon name="back" /> All flows
        </button>
      </div>
      <div className="mc-scroll">
        <div className="mc-flow-title" title={p.flowName}>
          {p.flowName || 'Untitled flow'}
        </div>
        {!p.saved && <p className="mc-hint" style={{ margin: '0 0 0.75rem' }}>Not saved yet. Running it, or Save, adds it to your flows.</p>}

        <button type="button" className="mc-row-btn" aria-current={designing} onClick={p.onDesign} style={{ marginBottom: '1rem' }}>
          <span className="mc-row-title">Design</span>
          <span className="mc-row-sub">
            <span>The agents and how they connect</span>
            {p.dirty && (
              <span className="mc-pill" data-tone="warn">
                unsaved
              </span>
            )}
          </span>
        </button>

        <div className="mc-section-head">
          <label className="mc-label" style={{ margin: 0 }}>
            Executions{mine.length ? ` · ${mine.length}` : ''}
          </label>
        </div>
        <button type="button" className="mc-btn mc-btn-primary" style={{ width: '100%', margin: '0.375rem 0 0.75rem' }} disabled={!p.canRun} onClick={p.onNewRun} title={p.canRun ? 'Start an execution of this flow' : 'Finish the flow first: see the message on the Run button'}>
          <Icon name="play" /> New run
        </button>

        {mine.length === 0 ? (
          <p className="mc-empty">No executions yet. Press New run, and each one will be kept here.</p>
        ) : (
          <ul className="mc-list">
            {mine.map((r) => (
              <li key={r.id}>
                <button type="button" className="mc-row-btn" aria-current={r.id === p.runId} onClick={() => p.onOpenRun(r.id)}>
                  <span className="mc-row-title">{r.goal}</span>
                  <span className="mc-row-sub">
                    <span>
                      {whenLabel(r.created)}
                      {r.tokens > 0 ? ` · ${fmtTokens(r.tokens)} tok` : ''}
                    </span>
                    <StatusPill status={r.status} />
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}
