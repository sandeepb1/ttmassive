// Executions as a table. The same panel is the Runs page (every flow's runs) and the Runs tab of an open flow (just that
// flow's), so a run is found the same way in both: by what it is doing now, or by words from its goal.

import { useMemo, useState, type ReactNode } from 'react';
import { Icon, StatusPill } from './bits';
import { menuOn, useLongPress, type MenuItem, type MenuState } from './ContextMenu';
import { RUN_GROUPS, filterRuns, fmtTokens, runCounts, usdLabel, whenLabel, type RunGroup } from './model';
import type { RunSummary } from './types';

interface Props {
  title: string;
  runs: RunSummary[];
  /** Show which flow each run belongs to (the page of every run). */
  showFlow: boolean;
  /** The execution being looked at, if any. */
  current?: string | null;
  /** A button for the head of the panel, such as New run. */
  action?: ReactNode;
  empty: string;
  onOpen: (r: RunSummary) => void;
  onRerun: (r: RunSummary) => void;
  onDelete: (r: RunSummary) => void;
  openMenu: (m: MenuState) => void;
  runMenu: (r: RunSummary) => MenuItem[];
}

export function RunsPanel(p: Props) {
  const [group, setGroup] = useState<RunGroup>('all');
  const [query, setQuery] = useState('');
  const press = useLongPress(p.openMenu);
  const counts = useMemo(() => runCounts(p.runs), [p.runs]);
  const shown = useMemo(() => filterRuns(p.runs, group, query), [p.runs, group, query]);
  const cols = p.showFlow ? 8 : 7;

  return (
    <div className="mc-page">
      <div className="mc-page-head">
        <h1>{p.title}</h1>
        <span className="mc-spacer" />
        {p.action}
      </div>
      <div className="mc-page-tools">
        <div className="mc-chips" role="group" aria-label="Show executions that are">
          {RUN_GROUPS.map((g) => (
            <button key={g.id} type="button" className="mc-chip" aria-pressed={group === g.id} onClick={() => setGroup(g.id)}>
              {g.label} <span className="mc-muted">{counts[g.id]}</span>
            </button>
          ))}
        </div>
        <input className="mc-field" type="search" value={query} placeholder="Search by goal or flow" aria-label="Search executions" onChange={(e) => setQuery(e.target.value)} />
      </div>

      <table className="mc-table" aria-label="Executions">
        <thead>
          <tr>
            <th className="mc-col-name">Goal</th>
            {p.showFlow && <th className="mc-col-flow">Flow</th>}
            <th className="mc-col-last">Status</th>
            <th className="mc-col-when">Started</th>
            <th className="mc-col-num">Tokens</th>
            <th className="mc-col-cost">API</th>
            <th className="mc-col-cost">Web</th>
            <th className="mc-col-act">
              <span className="mc-sronly">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {shown.map((r) => (
            <tr key={r.id} className="mc-tr" aria-current={r.id === p.current} {...menuOn(p.openMenu, () => p.runMenu(r))} {...press(() => p.runMenu(r))}>
              <td className="mc-col-name">
                <button type="button" className="mc-anchor mc-cellname" onClick={() => p.onOpen(r)}>
                  {r.goal}
                </button>
              </td>
              {p.showFlow && <td className="mc-col-flow mc-muted">{r.flowName}</td>}
              <td className="mc-col-last">
                <StatusPill status={r.status} />
              </td>
              <td className="mc-col-when mc-muted">{whenLabel(r.created)}</td>
              <td className="mc-col-num">{r.tokens > 0 ? fmtTokens(r.tokens) : '—'}</td>
              <td className="mc-col-cost">{(r.costUsd ?? 0) > 0 ? usdLabel(r.costUsd ?? 0) : '—'}</td>
              <td className="mc-col-cost">{(r.webUsd ?? 0) > 0 ? usdLabel(r.webUsd ?? 0) : '—'}</td>
              <td className="mc-col-act">
                <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Run “${r.goal}” again`} disabled={r.status.state === 'running' || r.status.state === 'grilling'} title="Start another with the same flow and brief" onClick={() => p.onRerun(r)}>
                  <Icon name="retry" />
                </button>
                <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Delete the execution “${r.goal}”`} disabled={r.status.state === 'running'} title={r.status.state === 'running' ? 'Stop it first' : 'Delete this execution'} onClick={() => p.onDelete(r)}>
                  <Icon name="trash" />
                </button>
              </td>
            </tr>
          ))}
          {shown.length === 0 && (
            <tr>
              <td colSpan={cols} className="mc-empty">
                {p.runs.length === 0 ? p.empty : 'No execution matches.'}
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
