// Small shared pieces: icons, the switch, the status pill.

import { memo, useMemo, type ReactNode } from 'react';
import { hasIcon, icon, type IconName } from '../icons';
import { statusLabel } from './model';
import { roleIconName } from './model';
import type { FlowNode, RunStatus } from './types';

export const Icon = memo(function Icon({ name }: { name: IconName }) {
  const html = useMemo(() => icon(name).outerHTML, [name]);
  return <span className="mc-ico" aria-hidden="true" style={{ display: 'inline-flex' }} dangerouslySetInnerHTML={{ __html: html }} />;
});

/** The icon for an agent's role. A role the server names an icon we do not have still gets one. */
export function iconFor(n: Pick<FlowNode, 'role' | 'custom'>): IconName {
  const name = roleIconName(n);
  return hasIcon(name) ? name : 'agent';
}

export function Switch({ on, onChange, label, hint, disabled }: { on: boolean; onChange: (v: boolean) => void; label: string; hint?: string; disabled?: boolean }) {
  return (
    <div>
      <button type="button" role="switch" aria-checked={on} className="mc-switch" disabled={disabled} onClick={() => onChange(!on)}>
        <span>{label}</span>
        <span className="mc-switch-track" />
      </button>
      {hint && <p className="mc-hint">{hint}</p>}
    </div>
  );
}

export function StatusPill({ status }: { status: RunStatus }) {
  const { text, tone } = statusLabel(status);
  return (
    <span className="mc-pill" data-tone={tone}>
      {text}
    </span>
  );
}

export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <div className="mc-section">
      <label className="mc-label">{label}</label>
      {children}
      {hint && <p className="mc-hint">{hint}</p>}
    </div>
  );
}

/** "3m ago", for the run list. */
export function ago(unixSeconds: number, now = Date.now()): string {
  const s = Math.max(0, Math.round(now / 1000 - unixSeconds));
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
