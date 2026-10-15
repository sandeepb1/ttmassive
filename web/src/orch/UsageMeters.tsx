// How full the plan windows are, with this run's own share picked out and the stop line marked.

import type { Gauge } from './model';

export function UsageMeters({ gauges }: { gauges: Gauge[] }) {
  return (
    <div className="mc-meters" aria-label="Plan usage">
      {gauges.map((g) => {
        const used = g.used ?? 0;
        const share = g.share ?? 0;
        return (
          <div key={g.engine} className="mc-meter" data-tone={g.tone} title={`Stops this run at ${g.stop}% of the window, or ${g.budget}% used by the run itself`}>
            <div className="mc-meter-head">
              <span>{g.label}</span>
              <b className="mc-mono">
                {g.used === null ? '–' : `${Math.round(used)}%`}
                {g.share !== null && g.share > 0 ? ` +${share < 10 ? share.toFixed(1) : Math.round(share)}` : ''}
              </b>
            </div>
            <div className="mc-meter-bar">
              <div className="mc-meter-fill" style={{ width: `${Math.min(100, used)}%` }} />
              {g.share !== null && g.share > 0 && <div className="mc-meter-share" style={{ left: `${Math.max(0, used - share)}%`, width: `${Math.min(100, share)}%` }} />}
              <div className="mc-meter-stop" style={{ left: `${g.stop}%` }} />
            </div>
          </div>
        );
      })}
    </div>
  );
}
