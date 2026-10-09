// A helper agent's whole transcript, read-only and live while it runs. It uses the same store and
// renderer as the chat, so messages, tool calls and diffs look the same; a helper that started
// helpers of its own shows them as cards that open in turn.

import { elapsedSecs, fmtElapsed, stateOf, workLine, type SubAgent } from './agents';
import { applyChat, newStore, type ChatResponse } from './chat-model';
import { renderItem, tickAgentTimes, type AgentCtx } from './chat-render';
import { el } from './dom';

const POLL_MS = 1000;
/** Once it has finished there is little to learn: look less often. */
const IDLE_POLL_MS = 5000;
const AGENTS_MS = 3000;
const STICK_PX = 80;

export interface AgentViewHooks {
  onUnauthorized: () => void;
  /** Open a helper this one started. */
  open: (a: SubAgent) => void;
}

export class AgentView {
  readonly el = el('div', 'agentview');
  private info = el('div', 'av-info');
  private scroller = el('div', 'av-scroll');
  private list = el('div', 'chat-list');
  private typing = el('div', 'chat-typing', 'Working…');
  private empty = el('div', 'chat-empty');
  private store = newStore();
  private nodes = new Map<string, HTMLElement>();
  private agents: SubAgent[] = [];
  private stuck = true;
  private pollTimer = 0;
  private agentsTimer = 0;
  private tickTimer = 0;
  private inflight = false;
  private disposed = false;
  private missing = false;
  private infoKey = '';
  private timeEl: HTMLElement | null = null;
  private ctx: AgentCtx = { list: () => this.agents, open: (a) => this.hooks.open(a) };

  constructor(
    private session: string,
    private agent: SubAgent,
    private hooks: AgentViewHooks,
  ) {
    this.agents = [agent];
    this.empty.textContent = 'Loading its transcript…';
    this.typing.hidden = true;
    this.scroller.append(this.list, this.typing, this.empty);
    this.scroller.addEventListener('scroll', () => {
      this.stuck = this.scroller.scrollHeight - this.scroller.scrollTop - this.scroller.clientHeight < STICK_PX;
    });
    this.el.append(this.info, this.scroller);
    this.renderInfo();
    void this.poll();
    void this.loadAgents();
    this.schedulePoll();
    this.agentsTimer = window.setInterval(() => void this.loadAgents(), AGENTS_MS);
    this.tickTimer = window.setInterval(() => {
      this.renderInfo();
      tickAgentTimes(this.list);
    }, 1000);
  }

  dispose() {
    this.disposed = true;
    window.clearTimeout(this.pollTimer);
    window.clearInterval(this.agentsTimer);
    window.clearInterval(this.tickTimer);
    this.el.remove();
  }

  /** The tab came back or the network returned. */
  nudge() {
    void this.poll();
    void this.loadAgents();
  }

  private get me(): SubAgent {
    return this.agents.find((a) => a.id === this.agent.id) ?? this.agent;
  }

  private schedulePoll() {
    window.clearTimeout(this.pollTimer);
    if (this.disposed) return;
    this.pollTimer = window.setTimeout(async () => {
      if (!document.hidden) await this.poll();
      this.schedulePoll();
    }, this.me.running ? POLL_MS : IDLE_POLL_MS);
  }

  private url(suffix: string): string {
    return `/api/sessions/${encodeURIComponent(this.session)}/${suffix}`;
  }

  private async loadAgents() {
    try {
      const r = await fetch(this.url('agents'));
      if (r.status === 401) return this.hooks.onUnauthorized();
      if (!r.ok || this.disposed) return;
      this.agents = (await r.json()) as SubAgent[];
      this.renderInfo();
    } catch {
      /* offline: keep what is showing */
    }
  }

  private async poll() {
    if (this.inflight || this.disposed) return;
    this.inflight = true;
    try {
      const q = new URLSearchParams();
      if (this.store.epoch !== null) q.set('epoch', String(this.store.epoch));
      q.set('after', String(this.store.seq));
      const r = await fetch(this.url(`agents/${encodeURIComponent(this.agent.id)}/chat?${q}`));
      if (r.status === 401) return this.hooks.onUnauthorized();
      if (this.disposed) return;
      if (r.status === 404) {
        this.missing = true;
        this.empty.hidden = false;
        this.empty.textContent = 'This helper’s log is no longer available.';
        return;
      }
      if (!r.ok) return;
      this.missing = false;
      this.apply((await r.json()) as ChatResponse);
    } catch {
      /* offline: the next poll tries again */
    } finally {
      this.inflight = false;
    }
  }

  private apply(resp: ChatResponse) {
    const wasStuck = this.stuck;
    const { reset, touched } = applyChat(this.store, resp);
    if (reset) {
      this.list.replaceChildren();
      this.nodes.clear();
    }
    for (const id of touched) {
      const pos = this.store.index.get(id);
      if (pos === undefined) continue;
      const node = renderItem(this.store.items[pos]!, undefined, this.ctx);
      const old = this.nodes.get(id);
      if (old) {
        if (old instanceof HTMLDetailsElement && node instanceof HTMLDetailsElement) node.open = old.open;
        old.replaceWith(node);
      } else {
        this.list.append(node);
      }
      this.nodes.set(id, node);
    }
    this.empty.hidden = this.store.items.length > 0;
    this.empty.textContent = this.me.has_log ? 'Nothing here yet.' : 'It has not written anything yet.';
    this.typing.hidden = !this.me.running;
    if (wasStuck) requestAnimationFrame(() => (this.scroller.scrollTop = this.scroller.scrollHeight));
  }

  /** Rebuild the bar only when something it shows changed; every second just move the clock. */
  private renderInfo() {
    const a = this.me;
    const state = stateOf(a);
    const key = `${state}|${a.kind}|${a.description}|${workLine(a)}`;
    if (key !== this.infoKey) {
      this.infoKey = key;
      const head = el('div', 'av-head');
      this.timeEl = el('span', 'av-time');
      head.append(
        el('span', 'acard-kind', a.kind === 'codex' ? 'Codex' : a.kind),
        el('span', `dot ${state === 'done' ? 'ready' : state === 'running' ? 'working' : 'idle'}`),
        el('span', 'av-state', state === 'done' ? 'Finished' : state === 'starting' ? 'Starting' : 'Running'),
        this.timeEl,
      );
      this.info.replaceChildren(head, el('div', 'av-desc', a.description), el('div', 'av-work', `${workLine(a)} · in ${this.session}`));
    }
    const secs = elapsedSecs(a, Math.floor(Date.now() / 1000));
    const text = secs === null ? '' : fmtElapsed(secs);
    if (this.timeEl && this.timeEl.textContent !== text) this.timeEl.textContent = text;
    this.typing.hidden = this.missing || !a.running;
  }
}
