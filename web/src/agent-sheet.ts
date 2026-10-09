// A helper agent's transcript in a sheet, for screens with no room for a second pane.

import { AgentView, type AgentViewHooks } from './agent-view';
import type { SubAgent } from './agents';
import { Sheet } from './sheet';

export class AgentSheet extends Sheet {
  private view: AgentView | null = null;

  constructor(
    host: HTMLElement,
    session: string,
    private helper: SubAgent,
    private viewHooks: AgentViewHooks,
  ) {
    super(host, session, `${helper.kind === 'codex' ? 'Codex' : helper.kind}: ${helper.description || 'helper'}`, viewHooks.onUnauthorized);
  }

  open() {
    this.tall();
    this.view = new AgentView(this.session, this.helper, this.viewHooks);
    this.body.replaceChildren(this.view.el);
    this.show();
  }

  protected onClose() {
    this.view?.dispose();
    this.view = null;
  }
}
