// The mode sheet: Plan or Build in one tap, and every other mode the agent offers below them.
// "Build" is whichever mode you were in before Plan, so Plan and back is two taps.

import type { ModeId, ModeInfo } from './chat-model';
import { el } from './dom';
import { Sheet, row } from './sheet';

export interface ModeState {
  mode: ModeId | null;
  modes: ModeInfo[];
  /** Where "Build" goes. */
  build: ModeInfo | null;
}

export interface ModeHooks {
  onUnauthorized: () => void;
  onChanged: (mode: ModeId) => void;
  /** About to switch to plan from this build mode: "Build" should come back to it. */
  onLeaving: (mode: ModeId) => void;
}

export class ModeSheet extends Sheet {
  constructor(
    host: HTMLElement,
    session: string,
    private hooks: ModeHooks,
  ) {
    super(host, session, 'Mode', hooks.onUnauthorized);
  }

  open(state: ModeState) {
    if (!this.gone) return;
    this.show();
    const plan = state.modes.find((m) => m.id === 'plan');
    const pair = el('div', 'mode-pair');
    if (plan) pair.append(this.big(plan, 'Plan', state));
    if (state.build) pair.append(this.big(state.build, `Build · ${state.build.label}`, state));

    const rows = el('div', 'sheet-list');
    for (const m of state.modes) {
      const b = row(m.label, { hint: m.hint, mark: m.id === state.mode ? '✓ now' : undefined, cls: m.id === 'bypass' ? 'sheet-danger' : m.id === state.mode ? 'current' : '' });
      b.addEventListener('click', () => void this.choose(m, state));
      rows.append(b);
    }
    this.body.replaceChildren(pair, el('div', 'sheet-label', 'All modes'), rows);
  }

  private big(m: ModeInfo, text: string, state: ModeState): HTMLButtonElement {
    // "Plan" is on when you are in plan; "Build" is on in any other mode.
    const on = m.id === 'plan' ? state.mode === 'plan' : state.mode !== 'plan' && state.mode !== null;
    const b = el('button', on ? 'mode-big on' : 'mode-big', text);
    b.type = 'button';
    b.addEventListener('click', () => void this.choose(m, state));
    return b;
  }

  private async choose(m: ModeInfo, state: ModeState) {
    if (this.busy) return;
    if (m.id === state.mode) return this.close();
    this.busy = true;
    if (m.id === 'plan' && state.mode) this.hooks.onLeaving(state.mode);
    this.message(`Switching to ${m.label}…`);
    const r = await this.call<{ mode: ModeId }>('mode', { mode: m.id });
    this.busy = false;
    if (!r.ok) return this.message(r.error, true);
    this.close();
    this.hooks.onChanged(r.data.mode);
  }
}
