// The flow as a vertical list, in the order the agents run. This is what a phone shows instead of the canvas.

import { Fragment } from 'react';
import { CardBody } from './AgentCard';
import { Icon } from './bits';
import { awaitingApproval, runOrder } from './model';
import type { Flow, Run } from './types';

export function NodeList({ flow, run, selected, onSelect }: { flow: Flow; run: Run | null; selected: string | null; onSelect: (id: string) => void }) {
  const awaiting = new Set(run ? awaitingApproval(run) : []);
  const nodes = runOrder(flow);
  return (
    <div className="mc-nodelist">
      {nodes.map((n, i) => {
        const nr = run?.nodes[n.id];
        return (
          <Fragment key={n.id}>
            {i > 0 && (
              <span className="mc-link" aria-hidden="true">
                <Icon name="arrow-down" />
              </span>
            )}
            <button
              type="button"
              className="mc-card"
              style={{ textAlign: 'left', cursor: 'pointer' }}
              data-engine={n.engine}
              data-status={nr?.status}
              data-waiting={awaiting.has(n.id) || undefined}
              data-selected={n.id === selected}
              onClick={() => onSelect(n.id)}
            >
              <CardBody node={n} run={nr} waiting={awaiting.has(n.id)} />
            </button>
          </Fragment>
        );
      })}
    </div>
  );
}
