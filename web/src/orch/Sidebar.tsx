// Runs and flows. Runs come first because that is what a person returns to.

import { useState } from 'react';
import { Icon, StatusPill, ago } from './bits';
import type { Flow, FlowLists, RunSummary } from './types';

interface Props {
  runs: RunSummary[];
  flows: FlowLists;
  runId: string | null;
  flowId: string | null;
  onNewRun: () => void;
  onOpenRun: (id: string) => void;
  onOpenFlow: (f: Flow, starter: boolean) => void;
  onNewFlow: () => void;
  onSuggest: () => void;
  onDuplicateFlow: (f: Flow) => void;
  onDeleteFlow: (f: Flow) => void;
}

export function Sidebar(p: Props) {
  const [tab, setTab] = useState<'runs' | 'flows'>('runs');
  return (
    <>
      <div className="mc-tabs" role="tablist">
        {(['runs', 'flows'] as const).map((t) => (
          <button key={t} type="button" role="tab" className="mc-tab" aria-selected={tab === t} onClick={() => setTab(t)}>
            {t === 'runs' ? 'Runs' : 'Flows'}
          </button>
        ))}
      </div>
      <div className="mc-scroll">
        {tab === 'runs' ? (
          <>
            <button type="button" className="mc-btn mc-btn-primary" style={{ width: '100%', marginBottom: '0.75rem' }} onClick={p.onNewRun}>
              <Icon name="play" /> New run
            </button>
            {p.runs.length === 0 ? (
              <p className="mc-empty">No runs yet. Pick or build a flow, then press New run.</p>
            ) : (
              <ul className="mc-list">
                {p.runs.map((r) => (
                  <li key={r.id}>
                    <button type="button" className="mc-row-btn" aria-current={r.id === p.runId} onClick={() => p.onOpenRun(r.id)}>
                      <span className="mc-row-title">{r.goal}</span>
                      <span className="mc-row-sub">
                        <span>
                          {r.flowName} · {ago(r.created)}
                        </span>
                        <StatusPill status={r.status} />
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </>
        ) : (
          <>
            <div className="mc-grid2" style={{ marginBottom: '0.75rem' }}>
              <button type="button" className="mc-btn" onClick={p.onNewFlow}>
                <Icon name="plus" /> New flow
              </button>
              <button type="button" className="mc-btn" onClick={p.onSuggest} title="Describe the work and let Sonnet design the team">
                <Icon name="wand" /> Suggest
              </button>
            </div>
            <label className="mc-label">Yours</label>
            {p.flows.saved.length === 0 ? (
              <p className="mc-empty">Flows you save appear here.</p>
            ) : (
              <ul className="mc-list">
                {p.flows.saved.map((f) => (
                  <li key={f.id} className="mc-li">
                    <button type="button" className="mc-row-btn" aria-current={f.id === p.flowId && !p.runId} onClick={() => p.onOpenFlow(f, false)}>
                      <span className="mc-row-title">{f.name}</span>
                      <span className="mc-row-sub">{f.nodes.length} agents</span>
                    </button>
                    <span style={{ display: 'flex' }}>
                      <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Duplicate ${f.name}`} title="Make a copy to edit" onClick={() => p.onDuplicateFlow(f)}>
                        <Icon name="files" />
                      </button>
                      <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Delete ${f.name}`} onClick={() => p.onDeleteFlow(f)}>
                        <Icon name="trash" />
                      </button>
                    </span>
                  </li>
                ))}
              </ul>
            )}
            <label className="mc-label" style={{ marginTop: '1rem' }}>Starters</label>
            <ul className="mc-list">
              {p.flows.starters.map((f) => (
                <li key={f.id}>
                  <button type="button" className="mc-row-btn" onClick={() => p.onOpenFlow(f, true)}>
                    <span className="mc-row-title">{f.name}</span>
                    <span className="mc-row-sub">{f.nodes.length} agents · a copy opens for you to edit</span>
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </>
  );
}
