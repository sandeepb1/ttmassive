// One open session: its header, the terminal, the chat, the prompt and restore bars and the key
// bar. The page shows one pane, or two side by side on a wide screen. Each pane has its own
// Chat/Terminal choice and its own agent accent; the sidebars describe whichever pane is focused.

import { AgentView } from './agent-view';
import type { SubAgent } from './agents';
import { PromptBar, StoppedBar } from './bars';
import { ChatView, type ChatInfo, type ChatPanelData } from './chat';
import { chooseView } from './chat-model';
import { el } from './dom';
import { icon } from './icons';
import type { Analysis } from './protocol';
import type { ChatActions } from './rightbar';
import { setAgentAccent } from './theme';
import { TerminalSession } from './term';
import { TermSearch } from './term-search';

export interface PaneEvents {
  onUnauthorized: () => void;
  /** The user touched this pane: make it the focused one. */
  onActivate: (pane: Pane) => void;
  /** Something the sidebars show changed. */
  onChange: (pane: Pane, what: 'name' | 'status' | 'info' | 'data' | 'agent' | 'progress') => void;
  onSplit: (pane: Pane) => void;
  /** Open the layout sheet: presets and saved workspaces. */
  onLayout: () => void;
  onClose: (pane: Pane) => void;
  onMenu: () => void;
  onPanel: () => void;
  /** A helper agent's card was opened in `session`. */
  onOpenAgent: (session: string, agent: SubAgent) => void;
}

const ACTIVITY: Record<Analysis['activity'], string> = {
  working: 'working…',
  waiting: 'waiting for you',
  idle: '',
  unknown: '',
};

const viewKey = (name: string) => `ttm.view.${name}`;

function savedView(name: string): string | null {
  try {
    return localStorage.getItem(viewKey(name));
  } catch {
    return null;
  }
}

/** A header button that does not take focus from the terminal (that would drop the soft keyboard). */
function headButton(cls: string, label: string, glyph: Parameters<typeof icon>[0], onClick: () => void): HTMLButtonElement {
  const b = el('button', `tb-btn ${cls}`);
  b.type = 'button';
  b.setAttribute('aria-label', label);
  b.title = label;
  b.append(icon(glyph));
  b.addEventListener('pointerdown', (e) => e.preventDefault());
  b.addEventListener('click', onClick);
  return b;
}

export class Pane {
  readonly el = el('section', 'pane');
  /** The session this pane shows (or, in agent mode, the session the helper belongs to). */
  name = '';
  /** What to call it: the session's name, or the helper's description. */
  title = '';
  /** The pane shows a helper agent's transcript instead of a session. */
  isAgent = false;
  view: 'chat' | 'term' = 'term';
  agent: 'claude' | 'codex' | null = null;
  info: ChatInfo | null = null;
  actions: ChatActions | null = null;
  data: ChatPanelData | null = null;
  /** The line under the name: connection, or what the agent is doing. */
  statusText = '';

  private nameEl = el('span', 'p-name');
  private statusEl = el('span', 'p-status');
  private toggle = el('div', 'p-viewtoggle');
  private termEl = el('div', 'p-term');
  private chatEl = el('div', 'p-chat');
  private stoppedEl = el('div', 'bar p-stopped');
  private promptEl = el('div', 'bar p-prompt');
  private keybarEl = el('div', 'p-keybar');
  private agentEl = el('div', 'p-agent');
  private agentView: AgentView | null = null;
  private term: TerminalSession | null = null;
  private chat: ChatView | null = null;
  private promptBar: PromptBar | null = null;
  private stoppedBar: StoppedBar | null = null;
  private focused = false;
  private historySearch: TermSearch;
  /** Whether the terminal draws on the GPU (browsers cap how many panes can). */
  private gpu = true;
  /** Whether this session's starting view has been picked yet. */
  private viewChosen = false;
  private connMsg = '';
  private activityMsg = '';
  private notice = '';
  private noticeTimer = 0;

  constructor(private events: PaneEvents) {
    this.el.hidden = false;
    this.chatEl.hidden = true;
    this.agentEl.hidden = true;
    this.stoppedEl.hidden = true;
    this.promptEl.hidden = true;
    this.keybarEl.setAttribute('role', 'toolbar');
    this.keybarEl.setAttribute('aria-label', 'Terminal keys');

    const title = el('div', 'p-title');
    this.statusEl.setAttribute('aria-live', 'polite');
    title.append(this.nameEl, this.statusEl);

    this.toggle.setAttribute('role', 'group');
    this.toggle.setAttribute('aria-label', 'View');
    this.toggle.hidden = true;
    for (const [view, label] of [['chat', 'Chat'], ['term', 'Terminal']] as const) {
      const b = el('button', undefined, label);
      b.type = 'button';
      b.dataset.view = view;
      b.addEventListener('pointerdown', (e) => e.preventDefault()); // don't close the keyboard
      b.addEventListener('click', () => this.setView(view, true));
      this.toggle.append(b);
    }

    const head = el('header', 'p-head');
    head.append(
      headButton('p-menu', 'Sessions', 'menu', () => events.onMenu()),
      title,
      this.toggle,
      headButton('p-split', 'Open another session in a new pane', 'columns', () => events.onSplit(this)),
      headButton('p-layout', 'Pane layout and workspaces', 'grid', () => events.onLayout()),
      headButton('p-close', 'Close this pane', 'close', () => events.onClose(this)),
      headButton('p-panel', 'Session details', 'panel-right', () => events.onPanel()),
    );

    this.historySearch = new TermSearch(this.termEl, {
      session: () => this.name,
      onUnauthorized: () => events.onUnauthorized(),
      refocus: () => {
        if (this.focused && this.view === 'term') this.term?.focus();
      },
    });
    this.el.append(head, this.termEl, this.chatEl, this.agentEl, this.stoppedEl, this.promptEl, this.keybarEl);
    // Capture, so a tap on a button inside still focuses the pane first.
    this.el.addEventListener('pointerdown', () => events.onActivate(this), { capture: true });
  }

  // ------------------------------------------------------------------ opening

  /** Show `name` in this pane, replacing whatever it showed. */
  open(name: string) {
    this.historySearch.reset();
    this.leaveAgent();
    this.term?.dispose();
    this.chat?.dispose();
    this.name = name;
    this.title = name;
    this.viewChosen = false;
    this.agent = null;
    this.info = null;
    this.actions = null;
    this.data = null;
    this.el.removeAttribute('data-agent');
    this.toggle.hidden = true;
    this.setView('term', false);
    this.nameEl.textContent = name;
    this.connMsg = '';
    this.activityMsg = '';
    this.renderStatus();
    this.events.onChange(this, 'name');

    const chat: ChatView = new ChatView(this.chatEl, name, {
      onUnauthorized: this.events.onUnauthorized,
      sheetHost: this.el,
      onOpenAgent: (a) => this.events.onOpenAgent(name, a),
      onAgent: (agent) => {
        if (this.chat !== chat || agent === this.agent) return;
        this.agent = agent;
        setAgentAccent(agent, this.el);
        this.events.onChange(this, 'agent');
      },
      onData: (data) => {
        if (this.chat !== chat) return;
        this.data = data;
        this.events.onChange(this, 'data');
      },
      onInfo: (info) => {
        if (this.chat !== chat) return;
        this.info = info;
        this.actions = {
          model: () => chat.openModelPicker(),
          mode: () => chat.openModeSheet(),
          more: () => chat.openMoreSheet(),
          resume: () => chat.openResumeSheet(),
        };
        this.events.onChange(this, 'info');
      },
      onEligible: (eligible) => {
        if (this.chat !== chat) return; // a session this pane has left
        this.toggle.hidden = !eligible;
        if (!eligible) return this.setView('term', false);
        if (!this.viewChosen) {
          this.viewChosen = true;
          this.setView(chooseView(savedView(name), true), false);
        }
      },
    });
    this.chat = chat;
    void chat.probe();

    const term = new TerminalSession(
      this.termEl,
      name,
      (s) => {
        this.connMsg = s;
        this.renderStatus();
      },
      this.events.onUnauthorized,
      this.keybarEl,
      {
        onStopped: (resumable) => this.stoppedBar?.show(resumable),
        onScreen: (a) => {
          this.promptBar?.render(a);
          this.activityMsg = a ? ACTIVITY[a.activity] : '';
          this.renderStatus();
        },
        onProgress: (kind) => {
          if ((this.el.dataset.progress ?? '') === (kind ?? '')) return;
          this.el.dataset.progress = kind ?? '';
          this.events.onChange(this, 'progress');
        },
      },
      this.gpu,
    );
    this.term = term;
    this.promptBar = new PromptBar(this.promptEl, (choice, id) => term.answer(choice, id));
    this.stoppedBar = new StoppedBar(this.stoppedEl, async (fresh) => {
      const r = await fetch(`/api/sessions/${encodeURIComponent(name)}/restore`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ fresh }),
      }).catch(() => null);
      if (!r) return 'Server unreachable';
      if (!r.ok) return await r.text();
      this.stoppedBar?.hide();
      term.reconnect();
      return null;
    });
    this.promptBar.render(null);
    this.stoppedBar.hide();
    term.connect();
    if (this.focused) term.focus();
  }

  /** Show a helper agent's transcript here, in place of a session. */
  openAgent(session: string, agent: SubAgent) {
    this.leaveAgent();
    this.term?.dispose();
    this.chat?.dispose();
    this.term = null;
    this.chat = null;
    this.promptBar = null;
    this.stoppedBar = null;
    this.isAgent = true;
    this.name = session;
    this.title = agent.description || agent.kind;
    this.agent = null;
    this.info = null;
    this.actions = null;
    this.data = null;
    this.el.removeAttribute('data-agent');
    this.el.classList.remove('chat-mode');
    this.el.classList.add('agent-mode');
    this.toggle.hidden = true;
    this.agentEl.hidden = false;
    this.nameEl.textContent = `${agent.kind === 'codex' ? 'Codex' : agent.kind}: ${this.title}`;
    this.connMsg = `helper in ${session}`;
    this.activityMsg = '';
    this.renderStatus();
    this.agentView = new AgentView(session, agent, {
      onUnauthorized: this.events.onUnauthorized,
      open: (child) => this.events.onOpenAgent(session, child),
    });
    this.agentEl.replaceChildren(this.agentView.el);
    this.events.onChange(this, 'name');
  }

  private leaveAgent() {
    this.agentView?.dispose();
    this.agentView = null;
    this.isAgent = false;
    this.agentEl.hidden = true;
    this.el.classList.remove('agent-mode');
  }

  dispose() {
    this.historySearch.reset(); // a closed pane must not leave its session in copy-mode
    this.agentView?.dispose();
    window.clearTimeout(this.noticeTimer);
    this.term?.dispose();
    this.chat?.dispose();
    this.term = null;
    this.chat = null;
    this.el.remove();
  }

  // ------------------------------------------------------------------ state

  /** Give the terminal the GPU renderer, or take it away to free a WebGL context. */
  setGpu(on: boolean) {
    this.gpu = on;
    this.term?.setGpu(on);
  }

  /** Open the history search, if this pane is showing a terminal. */
  openSearch() {
    if (this.view !== 'term' || this.isAgent || !this.term) return this.say('History search works in the terminal view.');
    this.historySearch.open();
  }

  setFocused(on: boolean) {
    this.focused = on;
    this.el.classList.toggle('focused', on);
    if (on && this.view === 'term') this.term?.focus();
  }

  /** Show the chat or the terminal. `remember` keeps the choice for this session. */
  setView(next: 'chat' | 'term', remember: boolean) {
    this.view = next;
    this.el.classList.toggle('chat-mode', next === 'chat');
    for (const b of this.toggle.querySelectorAll('button')) {
      b.setAttribute('aria-pressed', String(b.dataset.view === next));
    }
    if (next === 'chat') {
      this.chat?.show();
    } else {
      this.chat?.hide();
      if (this.focused) this.term?.focus();
    }
    if (remember) {
      try {
        localStorage.setItem(viewKey(this.name), next);
      } catch {
        /* the choice just isn't remembered */
      }
    }
  }

  /** Say something short in place of the status for a few seconds. */
  say(text: string) {
    this.notice = text;
    window.clearTimeout(this.noticeTimer);
    this.noticeTimer = window.setTimeout(() => {
      this.notice = '';
      this.renderStatus();
    }, 4000);
    this.renderStatus();
  }

  private renderStatus() {
    this.statusText = this.notice || this.connMsg || this.activityMsg;
    this.statusEl.textContent = this.statusText;
    this.events.onChange(this, 'status');
  }

  // ------------------------------------------------------------------ for the sidebars

  search(query: string) {
    return this.chat?.search(query) ?? [];
  }

  /** Bring a message into view, switching to the chat first if the terminal is showing. */
  showMessage(id: string) {
    const go = () => this.chat?.jumpTo(id);
    if (this.view === 'chat') return void go();
    this.setView('chat', true);
    window.setTimeout(go, 700); // the chat scrolls to the end once loaded: land after that
  }

  get terminalFont() {
    return this.term?.font ?? null;
  }

  setTerminalFont(px: number) {
    this.term?.setFont(px);
  }

  /** The tab came back or the network returned. */
  nudge() {
    this.term?.nudge();
    this.chat?.nudge();
    this.agentView?.nudge();
  }
}
