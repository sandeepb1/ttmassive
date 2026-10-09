// The chat view: a Claude or Codex conversation as messages, with a composer. It reads the
// agent's own log through `/api/sessions/{id}/chat` and types into the same tmux session, so
// the terminal view stays one tap away.

import { PromptBar } from './bars';
import {
  applyChat,
  buildTarget,
  chipFor,
  currentEffort,
  isRecorded,
  lastUserMessage,
  modeChipText,
  newStore,
  noteLeavingBuild,
  pendingQuestion,
  setOverride,
  type ChatResponse,
  type Todo,
} from './chat-model';
import { isAgentTool, type SubAgent } from './agents';
import { renderItem, tickAgentTimes, type AgentCtx, type RenderCtx } from './chat-render';
import { applySuggestion, attachmentToken, findTrigger, insertToken, isResumeCommand, removeToken, type Trigger } from './composer-text';
import { prepareImage } from './image';
import { ModeSheet } from './mode-sheet';
import { ModelPicker } from './model-picker';
import { MoreSheet } from './more-sheet';
import { ResumeSheet } from './resume-sheet';
import type { QuestionRequest } from './question-form';
import { el } from './dom';
import { tap } from './haptics';
import { filesChanged, searchItems, type ChangedFile, type Hit } from './chat-insights';

const POLL_MS = 1000;
/** While a session is not yet known to be chat-capable, look again this often. */
const PROBE_MS = 3000;
/** Within this many pixels of the bottom counts as "reading the latest". */
const STICK_PX = 80;
/** A sent message that never shows up in the log is dropped after this long. */
const PENDING_TTL_MS = 20_000;
const DRAFT_PREFIX = 'ttm.draft.';

export interface ChatHooks {
  onUnauthorized: () => void;
  /** Whether this session can show a chat at all (a Claude or Codex session). */
  onEligible: (eligible: boolean) => void;
  /** The model, mode or agent changed: for the session details in the right sidebar. */
  onInfo?: (info: ChatInfo) => void;
  /** A helper agent's card asked to open its transcript. */
  onOpenAgent?: (agent: SubAgent) => void;
  /** Which agent runs in this session (null: neither Claude nor Codex). Called on every poll. */
  onAgent?: (agent: 'claude' | 'codex' | null) => void;
  /** Files changed, to-dos and the plan changed: for the details panel. */
  onData?: (data: ChatPanelData) => void;
  /** Where sheets go. They must stay visible while the terminal is showing, so not the chat itself. */
  sheetHost?: HTMLElement;
}

/** What the right sidebar's Files and Plan tabs show. */
export interface ChatPanelData {
  files: ChangedFile[];
  todos: Todo[];
  plan: string | null;
  /** Every helper agent, including those whose card has scrolled out of the conversation. */
  agents: SubAgent[];
}

/** What the right sidebar shows about this conversation. */
export interface ChatInfo {
  agent: 'claude' | 'codex' | null;
  eligible: boolean;
  /** Model and reasoning level as the chip shows them, or ''. */
  model: string;
  /** Plan / accept-edits / etc. as the chip shows it, or ''. */
  mode: string;
  /** Started with permission checks skipped. */
  yolo: boolean;
}

/** Where each session's chat was scrolled to when you left it, so coming back doesn't lose your place. */
const scrollMemory = new Map<string, number>();

interface Suggestion {
  label: string;
  insert: string;
  detail: string;
}

interface Attachment {
  token: string;
  thumb: string;
}

interface Sent {
  text: string;
  seq: number;
  node: HTMLElement;
  at: number;
}

export class ChatView {
  private store = newStore();
  private nodes = new Map<string, HTMLElement>();
  private scroller = el('div', 'chat-scroll');
  private list = el('div', 'chat-list');
  private sentBox = el('div', 'chat-sent');
  private typing = el('div', 'chat-typing', 'Working…');
  private empty = el('div', 'chat-empty');
  private jump = el('button', 'chat-jump', '↓');
  private promptEl = el('div', 'bar chat-prompt');
  private err = el('div', 'chat-err');
  private input = el('textarea', 'c-input');
  private chip = el('button', 'c-chip');
  private stopBtn = el('button', 'c-stop', '■');
  private sendBtn = el('button', 'c-send', '↑');
  private attachBtn = el('button', 'c-attach', '+');
  private fileInput = el('input');
  private attachRow = el('div', 'attach-row');
  private suggestEl = el('div', 'suggest');
  /** Claude's suggested reply, offered as a chip above the box. */
  private replyChip = el('button', 'reply-chip');
  private note = el('div', 'chat-note');
  private noteTimer = 0;
  private agent: 'claude' | 'codex' | null = null;
  private picker: ModelPicker | null = null;
  private modeSheet: ModeSheet | null = null;
  private moreSheet: MoreSheet | null = null;
  private resumeSheet: ResumeSheet | null = null;
  private modeChip = el('button', 'c-mode');
  private moreBtn = el('button', 'c-more', '⋯');
  private suggestions: Suggestion[] = [];
  private activeSuggestion = 0;
  private trigger: Trigger | null = null;
  private suggestSeq = 0;
  private suggestTimer = 0;
  private attachments: Attachment[] = [];
  private promptBar: PromptBar;
  private ctx: RenderCtx = {
    answerQuestion: (req) => this.postQuestion(req),
    dismissQuestion: () => this.postQuestion({ dismiss: true }),
  };
  private timer = 0;
  private probeTimer = 0;
  private inflight = false;
  private shown = false;
  private disposed = false;
  private stuck = true;
  private eligible = false;
  private ready = false;
  private blocked = false;
  private working = false;
  private sending = false;
  private sent: Sent[] = [];
  private ro: ResizeObserver;
  private dataKey = '';
  private agentsKey = '';
  private agentCtx: AgentCtx = { list: () => this.store.agents, open: (a) => this.hooks.onOpenAgent?.(a) };
  private get sheetHost(): HTMLElement {
    return this.hooks.sheetHost ?? this.host;
  }

  constructor(
    private host: HTMLElement,
    private session: string,
    private hooks: ChatHooks,
  ) {
    const path = (suffix: string) => `/api/sessions/${encodeURIComponent(session)}/${suffix}`;
    this.path = path;

    this.typing.hidden = true;
    this.jump.type = 'button';
    this.jump.hidden = true;
    this.jump.setAttribute('aria-label', 'Jump to the latest message');
    this.scroller.append(this.list, this.sentBox, this.typing, this.empty);
    // Until the first response arrives, show the shape of a conversation instead of a blank page.
    this.empty.classList.add('skeleton');
    this.empty.replaceChildren(
      ...[86, 58, 72].map((w) => {
        const line = el('div', 'sk-line');
        line.style.width = `${w}%`;
        return line;
      }),
    );
    this.scroller.addEventListener('scroll', () => {
      this.stuck = this.scroller.scrollHeight - this.scroller.scrollTop - this.scroller.clientHeight < STICK_PX;
      if (this.stuck) this.jump.hidden = true;
    });
    this.jump.addEventListener('click', () => {
      this.stuck = true;
      this.jump.hidden = true;
      this.scrollToBottom();
    });
    // The keyboard opening or a rotation resizes the scroller: stay at the bottom if we were.
    this.ro = new ResizeObserver(() => {
      if (this.stuck) this.scrollToBottom();
    });
    this.ro.observe(this.scroller);

    this.promptBar = new PromptBar(this.promptEl, (choice, promptId) => this.answer(choice, promptId));

    const form = el('form', 'composer');
    this.input.rows = 1;
    this.input.placeholder = 'Message';
    this.input.setAttribute('aria-label', 'Message');
    this.input.enterKeyHint = 'enter';
    this.input.value = this.loadDraft();
    this.attachRow.hidden = true;
    this.input.addEventListener('input', () => {
      this.grow();
      this.saveDraft();
      this.syncControls();
      this.updateSuggest();
    });
    this.input.addEventListener('keyup', (e) => {
      if (e.key.startsWith('Arrow') && !this.suggestEl.hidden) return; // moving through the list
      if (e.key.startsWith('Arrow') || e.key === 'Home' || e.key === 'End') this.updateSuggest();
    });
    this.input.addEventListener('click', () => this.updateSuggest());
    this.input.addEventListener('keydown', (e) => {
      if (!this.suggestEl.hidden && !e.isComposing) {
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault();
          const n = this.suggestions.length;
          this.activeSuggestion = (this.activeSuggestion + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
          this.renderSuggestions();
          return;
        }
        if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey && matchMedia('(pointer: fine)').matches)) {
          e.preventDefault();
          this.pickSuggestion(this.activeSuggestion);
          return;
        }
        if (e.key === 'Escape') {
          e.preventDefault();
          this.closeSuggestions();
          return;
        }
      }
      // With a physical keyboard Enter sends and Shift+Enter breaks the line; on a touch
      // keyboard Enter always breaks the line and the arrow button sends.
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && matchMedia('(pointer: fine)').matches) {
        e.preventDefault();
        void this.send();
      }
    });

    this.chip.type = 'button';
    this.chip.hidden = true;
    this.chip.addEventListener('pointerdown', (e) => e.preventDefault());
    this.chip.addEventListener('click', () => this.openPicker());
    this.modeChip.type = 'button';
    this.modeChip.hidden = true;
    this.modeChip.addEventListener('pointerdown', (e) => e.preventDefault());
    this.modeChip.addEventListener('click', () => this.openModes());
    this.moreBtn.type = 'button';
    this.moreBtn.hidden = true;
    this.moreBtn.setAttribute('aria-label', 'More');
    this.moreBtn.addEventListener('pointerdown', (e) => e.preventDefault());
    this.moreBtn.addEventListener('click', () => this.openMore());
    this.stopBtn.type = 'button';
    this.stopBtn.hidden = true;
    this.stopBtn.setAttribute('aria-label', 'Stop');
    this.stopBtn.addEventListener('click', () => {
      tap();
      void this.answer('interrupt', null);
    });
    this.sendBtn.type = 'submit';
    this.sendBtn.addEventListener('pointerdown', (e) => e.preventDefault()); // keep the keyboard open
    this.sendBtn.setAttribute('aria-label', 'Send');

    this.suggestEl.hidden = true;
    this.attachBtn.type = 'button';
    this.attachBtn.setAttribute('aria-label', 'Attach an image');
    this.attachBtn.addEventListener('pointerdown', (e) => e.preventDefault());
    this.attachBtn.addEventListener('click', () => this.fileInput.click());
    this.fileInput.type = 'file';
    this.fileInput.accept = 'image/*';
    this.fileInput.hidden = true;
    this.fileInput.addEventListener('change', () => {
      const file = this.fileInput.files?.[0];
      this.fileInput.value = ''; // so picking the same photo again still fires
      if (file) void this.attach(file);
    });

    this.replyChip.type = 'button';
    this.replyChip.hidden = true;
    this.replyChip.addEventListener('click', () => this.useSuggestedReply());
    const row = el('div', 'composer-row');
    row.append(this.attachBtn, this.chip, this.modeChip, el('span', 'c-spacer'), this.moreBtn, this.stopBtn, this.sendBtn);
    form.append(this.suggestEl, this.replyChip, this.note, this.err, this.attachRow, this.input, row, this.fileInput);
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      void this.send();
    });

    // The jump button floats over the messages, just above the composer.
    const main = el('div', 'chat-main');
    main.append(this.scroller, this.jump);
    host.replaceChildren(main, this.promptEl, form);
    // A new session only becomes chat-capable once its agent reports a conversation id.
    this.probeTimer = window.setInterval(() => {
      if (!this.eligible && !this.shown && !document.hidden) void this.poll();
    }, PROBE_MS);
    this.grow();
    this.syncControls();
    this.renderEmpty();
  }

  private path: (suffix: string) => string;

  // ----------------------------------------------------------- lifecycle

  /** Read the conversation once, even while hidden, to learn whether chat applies. */
  async probe() {
    await this.poll();
  }

  show() {
    this.shown = true;
    this.host.hidden = false;
    this.grow(); // a draft written while hidden needs its height now that it can be measured
    const saved = scrollMemory.get(this.session);
    this.stuck = saved === undefined;
    void this.poll().then(() => (saved === undefined ? this.scrollToBottom() : this.restoreScroll(saved)));
    window.clearInterval(this.timer);
    this.timer = window.setInterval(() => {
      if (!document.hidden) void this.poll();
    }, POLL_MS);
  }

  hide() {
    this.remember();
    this.shown = false;
    this.host.hidden = true;
    window.clearInterval(this.timer);
  }

  /** The tab came back or the network returned. */
  nudge() {
    if (this.shown) void this.poll();
  }

  dispose() {
    if (this.shown) this.remember();
    this.disposed = true;
    window.clearInterval(this.timer);
    window.clearInterval(this.probeTimer);
    window.clearTimeout(this.suggestTimer);
    window.clearTimeout(this.noteTimer);
    this.picker?.close();
    this.modeSheet?.close();
    this.moreSheet?.close();
    this.resumeSheet?.close();
    for (const a of this.attachments) URL.revokeObjectURL(a.thumb);
    this.ro.disconnect();
    this.host.replaceChildren();
  }

  // ------------------------------------------------------------- polling

  private async poll() {
    if (this.inflight || this.disposed) return;
    this.inflight = true;
    try {
      const q = new URLSearchParams();
      if (this.store.epoch !== null) {
        q.set('epoch', String(this.store.epoch));
        q.set('after', String(this.store.seq));
      }
      const r = await fetch(`${this.path('chat')}?${q}`);
      if (r.status === 401) return this.hooks.onUnauthorized();
      if (!r.ok) return;
      this.apply((await r.json()) as ChatResponse);
    } catch {
      /* offline for a moment: keep showing what we have */
    } finally {
      this.inflight = false;
    }
  }

  private apply(resp: ChatResponse) {
    if (this.disposed) return;
    if (resp.eligible !== this.eligible) {
      this.eligible = resp.eligible;
      this.hooks.onEligible(resp.eligible);
    }
    this.ready = resp.ready;
    this.agent = resp.agent;
    this.hooks.onAgent?.(resp.agent);

    const { reset, touched } = applyChat(this.store, resp);
    if (reset) {
      this.list.replaceChildren();
      this.nodes.clear();
    }
    const wasStuck = this.stuck;
    let added = false;
    for (const id of touched) {
      const pos = this.store.index.get(id);
      if (pos === undefined) continue;
      const node = renderItem(this.store.items[pos], this.ctx, this.agentCtx);
      const old = this.nodes.get(id);
      if (old) {
        // Keep a tool call or thought open if you opened it.
        if (old instanceof HTMLDetailsElement && node instanceof HTMLDetailsElement) node.open = old.open;
        old.replaceWith(node);
      } else {
        this.list.append(node);
        added = true;
      }
      this.nodes.set(id, node);
    }
    this.refreshAgentCards();
    tickAgentTimes(this.list);
    this.settleSent();

    const scr = resp.screen;
    // A question from the agent's own question tool has its own card: don't also offer its menu
    // as buttons in the bar.
    const asking = pendingQuestion(this.store.items) !== undefined;
    this.blocked = scr?.prompt != null || asking;
    this.working = !this.blocked && scr?.activity === 'working';
    // Only questions go in the bar here: "working" has its own indicator and Stop button.
    this.promptBar.render(scr && !asking ? { ...scr, activity: scr.prompt ? scr.activity : 'idle' } : null);
    this.typing.hidden = !this.working;

    this.refreshChip();
    this.refreshMode();
    this.syncControls();
    this.renderEmpty();
    this.emitData();

    if (wasStuck) this.scrollToBottom();
    else if (added) this.jump.hidden = false;
  }

  /** Redraw the helper agents' cards when what they show changed. A running helper's log changes
   *  every second, which the cards do not show, so that alone does not count. */
  private refreshAgentCards() {
    const key = JSON.stringify(this.store.agents.map((a) => ({ ...a, last: a.running ? null : a.last })));
    if (key === this.agentsKey) return;
    this.agentsKey = key;
    for (const it of this.store.items) {
      if (it.kind !== 'tool' || !isAgentTool(it.tool)) continue;
      const old = this.nodes.get(it.id);
      if (!old) continue;
      const node = renderItem(it, this.ctx, this.agentCtx);
      if (old instanceof HTMLDetailsElement && node instanceof HTMLDetailsElement) node.open = old.open;
      old.replaceWith(node);
      this.nodes.set(it.id, node);
    }
  }

  /** Tell the details panel about files, to-dos and the plan, but only when they changed. */
  private emitData() {
    const { todos, plan, seq, agents } = this.store;
    // A running helper's log time changes every second and is not shown here: leave it out.
    const agentKey = JSON.stringify(agents.map((a) => ({ ...a, last: a.running ? null : a.last })));
    const key = `${seq}|${JSON.stringify(todos)}|${plan?.length ?? -1}|${plan?.slice(0, 40) ?? ''}|${agentKey}`;
    if (key === this.dataKey) return;
    this.dataKey = key;
    this.hooks.onData?.({ files: filesChanged(this.store.items), todos, plan, agents });
  }

  /** Messages containing `query`, newest first. */
  search(query: string): Hit[] {
    return searchItems(this.store.items, query);
  }

  /** Scroll a message into view and flash it. False when it is not on screen (the terminal is showing). */
  jumpTo(id: string): boolean {
    const node = this.nodes.get(id);
    if (!node || !this.shown) return false;
    this.stuck = false;
    node.scrollIntoView({ block: 'center', behavior: 'smooth' });
    node.classList.remove('flash');
    void node.offsetWidth; // restart the animation if it is already running
    node.classList.add('flash');
    window.setTimeout(() => node.classList.remove('flash'), 1800);
    return true;
  }

  private emitInfo() {
    this.hooks.onInfo?.({
      agent: this.agent,
      eligible: this.eligible,
      model: chipFor(this.store),
      mode: modeChipText(this.store),
      yolo: this.store.yolo,
    });
  }

  private refreshChip() {
    const label = chipFor(this.store);
    this.chip.textContent = label;
    this.chip.hidden = label === '';
    this.chip.title = this.agent ? 'Change model or effort' : "Switching the model isn't available here";
    this.emitInfo();
  }

  private refreshMode() {
    const text = modeChipText(this.store);
    this.modeChip.textContent = text;
    this.modeChip.hidden = text === '';
    this.modeChip.className = `c-mode m-${this.store.mode ?? 'none'}${this.store.yolo ? ' yolo' : ''}`;
    this.modeChip.title = this.store.yolo ? 'Started with permission checks skipped. Tap to change mode.' : 'Change mode';
    this.moreBtn.hidden = this.agent === null;
    this.emitInfo();
  }

  openModelPicker() {
    this.openPicker();
  }
  openModeSheet() {
    this.openModes();
  }
  openMoreSheet() {
    this.openMore();
  }
  openResumeSheet() {
    this.openResume();
  }

  private openModes() {
    this.modeSheet ??= new ModeSheet(this.sheetHost, this.session, {
      onUnauthorized: this.hooks.onUnauthorized,
      onChanged: (mode) => {
        const label = this.store.modes.find((m) => m.id === mode)?.label ?? mode;
        this.say(`Mode: ${label}`);
        void this.poll(); // the chip follows the footer
      },
      onLeaving: (mode) => noteLeavingBuild(this.store, mode),
    });
    this.modeSheet.open({ mode: this.store.mode, modes: this.store.modes, build: buildTarget(this.store) });
  }

  private openMore() {
    const agent = this.agent;
    if (agent !== 'claude' && agent !== 'codex') return;
    this.moreSheet ??= new MoreSheet(this.sheetHost, this.session, {
      onUnauthorized: this.hooks.onUnauthorized,
      onEffort: (level) => {
        setOverride(this.store, { effort: level });
        this.refreshChip();
      },
      onSay: (text) => this.say(text),
      onReuse: () => this.reuseLast(),
    });
    this.moreSheet.open({
      agent,
      effort: currentEffort(this.store),
      hasLast: lastUserMessage(this.store.items) !== null,
    });
  }

  private openResume() {
    this.resumeSheet ??= new ResumeSheet(this.sheetHost, this.session, {
      onUnauthorized: this.hooks.onUnauthorized,
      onResumed: (text) => {
        this.say(text);
        this.stuck = true;
        void this.poll();
      },
    });
    void this.resumeSheet.open();
  }

  /** Put your last message back in the box, to change and send as a new message. */
  private reuseLast() {
    const text = lastUserMessage(this.store.items);
    if (text === null) return;
    this.input.value = text;
    this.grow();
    this.saveDraft();
    this.syncControls();
    this.input.focus();
    this.input.setSelectionRange(text.length, text.length);
    this.say('Edit it and send it as a new message.');
  }

  /** A short confirmation above the composer. */
  private say(text: string) {
    this.note.textContent = text;
    window.clearTimeout(this.noteTimer);
    this.noteTimer = window.setTimeout(() => (this.note.textContent = ''), 6000);
  }

  private openPicker() {
    const agent = this.agent;
    if (agent !== 'claude' && agent !== 'codex') return this.say("Switching the model isn't available here.");
    this.picker ??= new ModelPicker(this.sheetHost, this.session, agent, {
      onUnauthorized: this.hooks.onUnauthorized,
      onChanged: (change, message) => {
        setOverride(this.store, change);
        this.refreshChip();
        this.say(message);
      },
    });
    void this.picker.open(this.store.effortOverride?.level ?? this.store.effort);
  }

  private renderEmpty() {
    const none = this.store.items.length === 0 && this.sent.length === 0;
    this.empty.hidden = !none;
    this.empty.classList.remove('skeleton');
    this.empty.textContent = !this.eligible
      ? 'Chat is only available for Claude and Codex sessions.'
      : !this.ready
        ? 'Waiting for the first message. The conversation appears here once the agent has started it.'
        : 'No messages yet.';
  }

  /** Note where the conversation is scrolled to (or that it is at the end) while it can be measured. */
  private remember() {
    if (!this.shown) return;
    if (this.stuck) scrollMemory.delete(this.session);
    else scrollMemory.set(this.session, this.scroller.scrollTop);
  }

  private restoreScroll(top: number) {
    requestAnimationFrame(() => {
      this.scroller.scrollTop = top;
      this.stuck = this.scroller.scrollHeight - this.scroller.scrollTop - this.scroller.clientHeight < STICK_PX;
      this.jump.hidden = this.stuck;
    });
  }

  private scrollToBottom() {
    requestAnimationFrame(() => {
      this.scroller.scrollTop = this.scroller.scrollHeight;
    });
  }

  // ------------------------------------------------------------ composer

  private grow() {
    this.input.style.height = 'auto';
    // While the view is hidden nothing is laid out and scrollHeight is 0: leave the height alone
    // rather than pinning the box shut.
    if (this.input.scrollHeight > 0) this.input.style.height = `${Math.min(this.input.scrollHeight, 160)}px`;
  }

  /** Put Claude's suggested reply in the box, to send as it is or change first. */
  private useSuggestedReply() {
    const text = this.store.suggestion;
    if (!text) return;
    this.input.value = text;
    this.grow();
    this.saveDraft();
    this.syncControls();
    this.input.focus();
    this.input.setSelectionRange(text.length, text.length);
  }

  private syncControls() {
    const empty = this.input.value.trim() === '';
    const reply = this.store.suggestion;
    this.replyChip.hidden = !reply || !empty || this.blocked;
    if (reply) this.replyChip.textContent = `↳ ${reply}`;
    this.sendBtn.disabled = empty || this.blocked || this.sending;
    this.stopBtn.hidden = !this.working;
    this.input.placeholder = this.blocked ? 'Answer the question above first' : 'Message';
  }

  private loadDraft(): string {
    try {
      return localStorage.getItem(DRAFT_PREFIX + this.session) ?? '';
    } catch {
      return '';
    }
  }

  private saveDraft() {
    try {
      if (this.input.value) localStorage.setItem(DRAFT_PREFIX + this.session, this.input.value);
      else localStorage.removeItem(DRAFT_PREFIX + this.session);
    } catch {
      /* a draft that is not kept is not worth an error */
    }
  }

  // --------------------------------------------------------- suggestions

  private updateSuggest() {
    const t = findTrigger(this.input.value, this.input.selectionStart ?? this.input.value.length);
    this.trigger = t;
    window.clearTimeout(this.suggestTimer);
    if (!t || this.blocked) return this.closeSuggestions();
    this.suggestTimer = window.setTimeout(() => void this.fetchSuggestions(t), 120);
  }

  private async fetchSuggestions(t: Trigger) {
    const seq = ++this.suggestSeq;
    const q = new URLSearchParams({ kind: t.kind, q: t.query });
    const r = await fetch(`${this.path('suggest')}?${q}`).catch(() => null);
    if (r?.status === 401) return this.hooks.onUnauthorized();
    if (!r?.ok || seq !== this.suggestSeq) return; // an answer to something no longer typed
    this.suggestions = (await r.json()) as Suggestion[];
    this.activeSuggestion = 0;
    this.renderSuggestions();
  }

  private renderSuggestions() {
    if (this.suggestions.length === 0 || !this.trigger) return this.closeSuggestions();
    this.suggestEl.hidden = false;
    this.suggestEl.replaceChildren(
      ...this.suggestions.map((s, i) => {
        const b = el('button', i === this.activeSuggestion ? 'sg active' : 'sg');
        b.type = 'button';
        b.append(el('span', 'sg-label', s.label));
        if (s.detail) b.append(el('span', 'sg-detail', s.detail));
        b.addEventListener('pointerdown', (e) => e.preventDefault()); // keep the keyboard open
        b.addEventListener('click', () => this.pickSuggestion(i));
        return b;
      }),
    );
  }

  private closeSuggestions() {
    this.suggestSeq++; // an answer still on its way is no longer wanted
    this.suggestions = [];
    this.suggestEl.hidden = true;
    this.suggestEl.replaceChildren();
  }

  private pickSuggestion(i: number) {
    const s = this.suggestions[i];
    const t = this.trigger;
    if (!s || !t) return;
    const next = applySuggestion(this.input.value, t, s.insert);
    this.input.value = next.text;
    this.input.setSelectionRange(next.cursor, next.cursor);
    this.closeSuggestions();
    this.grow();
    this.saveDraft();
    this.syncControls();
    this.input.focus();
  }

  // --------------------------------------------------------- attachments

  private async attach(file: File) {
    this.err.textContent = '';
    try {
      const blob = await prepareImage(file);
      const r = await fetch(this.path('upload'), {
        method: 'POST',
        headers: { 'content-type': blob.type || 'application/octet-stream' },
        body: blob,
      });
      if (r.status === 401) return this.hooks.onUnauthorized();
      if (!r.ok) return this.fail((await r.text()) || 'Could not upload the image');
      const { path } = (await r.json()) as { path: string };
      const token = attachmentToken(path);
      const at = insertToken(this.input.value, this.input.selectionStart ?? this.input.value.length, token);
      this.input.value = at.text;
      this.input.setSelectionRange(at.cursor, at.cursor);
      this.attachments.push({ token, thumb: URL.createObjectURL(blob) });
      this.renderAttachments();
      this.grow();
      this.saveDraft();
      this.syncControls();
    } catch (e) {
      this.fail(e instanceof Error ? e.message : 'Could not attach the image');
    }
  }

  private renderAttachments() {
    this.attachRow.replaceChildren(
      ...this.attachments.map((a) => {
        const chip = el('span', 'att');
        const img = el('img');
        img.src = a.thumb;
        img.alt = 'Attached image';
        const x = el('button', 'att-x', '✕');
        x.type = 'button';
        x.setAttribute('aria-label', 'Remove the image');
        x.addEventListener('click', () => {
          this.input.value = removeToken(this.input.value, a.token);
          URL.revokeObjectURL(a.thumb);
          this.attachments = this.attachments.filter((o) => o !== a);
          this.renderAttachments();
          this.grow();
          this.saveDraft();
          this.syncControls();
        });
        chip.append(img, x);
        return chip;
      }),
    );
    this.attachRow.hidden = this.attachments.length === 0;
  }

  private fail(message: string) {
    this.err.textContent = message;
    this.sending = false;
    this.syncControls();
  }

  private async send() {
    tap();
    const text = this.input.value.trim();
    if (!text || this.blocked || this.sending) return;
    // `/resume` opens a list in the agent: show that list here instead of sending a message.
    if (isResumeCommand(text) && (this.agent === 'claude' || this.agent === 'codex')) {
      this.input.value = '';
      this.closeSuggestions();
      this.grow();
      this.saveDraft();
      this.syncControls();
      this.openResume();
      return;
    }
    this.sending = true;
    this.syncControls();
    const r = await fetch(this.path('message'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ text }),
    }).catch(() => null);
    if (!r) return this.fail('Server unreachable');
    if (r.status === 401) return this.hooks.onUnauthorized();
    if (!r.ok) return this.fail((await r.text()) || 'Could not send');
    this.err.textContent = '';
    this.input.value = '';
    for (const a of this.attachments) URL.revokeObjectURL(a.thumb);
    this.attachments = [];
    this.renderAttachments();
    this.closeSuggestions();
    this.grow();
    this.saveDraft();
    this.sending = false;
    this.syncControls();
    this.showSent(text);
    this.stuck = true;
    void this.poll();
  }

  /** Show a message right away; the log records it a moment later and this copy then goes. */
  private showSent(text: string) {
    const node = renderItem({ id: 'pending', seq: 0, ts: null, kind: 'user', text, images: 0 });
    node.classList.add('pending');
    this.sentBox.append(node);
    this.sent.push({ text, seq: this.store.seq, node, at: Date.now() });
    this.renderEmpty();
    this.scrollToBottom();
  }

  private settleSent() {
    const now = Date.now();
    this.sent = this.sent.filter((s) => {
      const done = isRecorded(this.store.items, s.text, s.seq) || now - s.at > PENDING_TTL_MS;
      if (done) s.node.remove();
      return !done;
    });
  }

  /** Type answers into the agent's question form, or dismiss it. Returns an error, or null. */
  private async postQuestion(body: QuestionRequest | { dismiss: true }): Promise<string | null> {
    const r = await fetch(this.path('question'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    }).catch(() => null);
    if (!r) return 'Server unreachable';
    if (r.status === 401) {
      this.hooks.onUnauthorized();
      return null;
    }
    if (!r.ok) return (await r.text()) || 'Could not send the answer';
    void this.poll();
    return null;
  }

  /** Answer the question on screen, or interrupt. Returns an error message, or null on success. */
  private async answer(choice: string, promptId: string | null): Promise<string | null> {
    const r = await fetch(this.path('answer'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ choice, prompt_id: promptId }),
    }).catch(() => null);
    if (!r) return 'Server unreachable';
    if (r.status === 401) {
      this.hooks.onUnauthorized();
      return null;
    }
    if (r.ok) {
      void this.poll();
      return null;
    }
    return r.status === 409 ? 'The question changed. Check the screen.' : await r.text();
  }
}
