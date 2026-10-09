// The right sidebar: what is going on in the open session. Four tabs: Overview (model, mode,
// conversation actions, plan usage), Files (what the agent changed), Plan (its to-do list and
// plan) and Images (what you attached). A search box pinned above them looks through the
// conversation; picking a result jumps to the message.

import type { ChatInfo, ChatPanelData } from './chat';
import type { Hit } from './chat-insights';
import { renderDiff, renderMarkdown } from './chat-render';
import type { Todo } from './chat-model';
import { el } from './dom';
import { icon, type IconName } from './icons';

export interface ChatActions {
  model: () => void;
  mode: () => void;
  more: () => void;
  resume: () => void;
}

export interface RightHooks {
  collapse: () => void;
  refreshUsage: () => void;
  /** Messages in the open conversation containing the text, newest first. */
  search: (query: string) => Hit[];
  /** Show a message from the results in the chat. */
  jump: (id: string) => void;
}

type Tab = 'overview' | 'files' | 'plan' | 'images';

const TABS: { id: Tab; label: string; icon: IconName }[] = [
  { id: 'overview', label: 'Overview', icon: 'info' },
  { id: 'files', label: 'Files', icon: 'files' },
  { id: 'plan', label: 'Plan', icon: 'todo' },
  { id: 'images', label: 'Images', icon: 'image' },
];

const TAB_KEY = 'ttm.rightTab';
const ATTACH_POLL_MS = 5000;
const KIND_LABEL: Record<string, string> = {
  user: 'You',
  assistant: 'Agent',
  thinking: 'Thinking',
  tool: 'Tool',
  command: 'Command',
  notice: 'Note',
  question: 'Question',
};

interface Attachment {
  name: string;
  url: string;
  size: number;
  ts: number;
}

function savedTab(): Tab {
  try {
    const t = localStorage.getItem(TAB_KEY);
    if (TABS.some((x) => x.id === t)) return t as Tab;
  } catch {
    /* not remembered */
  }
  return 'overview';
}

export class RightBar {
  private title = el('strong', 'rb-name');
  private badge = el('span', 'rb-badge');
  private status = el('div', 'rb-status');
  private rows = el('div', 'rb-rows');
  private search = el('input', 'rb-search-input');
  private clear = el('button', 'rb-search-clear');
  private tabBtns = new Map<Tab, HTMLButtonElement>();
  private tabBadges = new Map<Tab, HTMLElement>();
  private panels = new Map<Tab, HTMLElement>();
  private results = el('div', 'rb-results');
  private info: ChatInfo | null = null;
  private actions: ChatActions | null = null;
  private data: ChatPanelData | null = null;
  private attachments: Attachment[] = [];
  private session = '';
  private tab: Tab = savedTab();
  private query = '';
  private searchTimer = 0;
  private attachTimer = 0;
  private active = false;
  /** Files whose diff is open, kept across re-renders. */
  private openFiles = new Set<string>();

  constructor(
    private root: HTMLElement,
    usageBody: HTMLElement,
    private hooks: RightHooks,
  ) {
    this.root.classList.add('sidebar', 'sidebar-right');
    this.root.setAttribute('aria-label', 'Session details');

    const head = el('div', 'sb-head');
    const hide = el('button', 'icon sb-collapse');
    hide.type = 'button';
    hide.setAttribute('aria-label', 'Hide details');
    hide.append(icon('panel-right'));
    hide.addEventListener('click', () => this.hooks.collapse());
    head.append(el('strong', 'sb-brand', 'Details'), el('span', 'sb-spacer'), hide);

    // Search, pinned above the tabs.
    const box = el('div', 'rb-search');
    this.search.type = 'search';
    this.search.placeholder = 'Search this conversation';
    this.search.setAttribute('aria-label', 'Search this conversation');
    this.search.enterKeyHint = 'search';
    this.search.autocomplete = 'off';
    this.search.addEventListener('input', () => {
      window.clearTimeout(this.searchTimer);
      this.searchTimer = window.setTimeout(() => this.setQuery(this.search.value), 150);
    });
    this.clear.type = 'button';
    this.clear.hidden = true;
    this.clear.setAttribute('aria-label', 'Clear search');
    this.clear.append(icon('close'));
    this.clear.addEventListener('click', () => {
      this.search.value = '';
      this.setQuery('');
    });
    const mag = icon('search', 'icon-svg rb-search-icon');
    box.append(mag, this.search, this.clear);

    // Tabs.
    const tabs = el('div', 'rb-tabs');
    tabs.setAttribute('role', 'tablist');
    for (const t of TABS) {
      const b = el('button', 'rb-tab');
      b.type = 'button';
      b.setAttribute('role', 'tab');
      b.setAttribute('aria-label', t.label);
      b.title = t.label;
      const badge = el('span', 'rb-tab-badge');
      badge.hidden = true;
      b.append(icon(t.icon), el('span', 'rb-tab-label', t.label), badge);
      b.addEventListener('click', () => this.select(t.id));
      this.tabBtns.set(t.id, b);
      this.tabBadges.set(t.id, badge);
      tabs.append(b);
    }

    // Overview panel: the session card, conversation actions and usage.
    const card = el('div', 'rb-card');
    const top = el('div', 'rb-card-top');
    top.append(this.title, this.badge);
    card.append(top, this.status);
    const convo = el('section', 'rb-section');
    convo.append(el('div', 'drawer-label', 'Conversation'), this.rows);
    const usage = el('section', 'rb-section');
    const refresh = el('button', 'chip rb-refresh', 'Refresh');
    refresh.type = 'button';
    refresh.addEventListener('click', () => this.hooks.refreshUsage());
    const uhead = el('div', 'rb-uhead');
    uhead.append(el('div', 'drawer-label', 'Plan usage'), refresh);
    usage.append(uhead, usageBody);
    const overview = el('div', 'rb-panel');
    overview.append(card, convo, usage);

    const body = el('div', 'rb-body');
    for (const t of TABS) {
      const p = t.id === 'overview' ? overview : el('div', 'rb-panel');
      p.setAttribute('role', 'tabpanel');
      this.panels.set(t.id, p);
      body.append(p);
    }
    this.results.hidden = true;
    body.append(this.results);

    this.root.append(head, box, tabs, body);
    this.render();
  }

  // ------------------------------------------------------------------ feeding it

  setSession(name: string) {
    this.session = name;
    this.title.textContent = name;
    this.info = null;
    this.actions = null;
    this.data = null;
    this.attachments = [];
    this.openFiles.clear();
    this.search.value = '';
    this.setQuery('');
    this.render();
    if (this.active) void this.loadAttachments();
  }

  /** The connection or activity line, e.g. "working…". */
  setStatus(text: string) {
    this.status.textContent = text;
  }

  setInfo(info: ChatInfo | null, actions: ChatActions | null) {
    this.info = info;
    this.actions = actions;
    this.renderOverview();
  }

  setData(data: ChatPanelData | null) {
    this.data = data;
    this.renderFiles();
    this.renderPlan();
    this.renderBadges();
    if (this.query) this.runSearch(); // new messages may match
  }

  /** The sidebar is on screen: keep the image list fresh while it is. */
  setActive(on: boolean) {
    if (on === this.active) return;
    this.active = on;
    window.clearInterval(this.attachTimer);
    if (on) {
      void this.loadAttachments();
      this.attachTimer = window.setInterval(() => void this.loadAttachments(), ATTACH_POLL_MS);
    }
  }

  // ------------------------------------------------------------------ tabs

  private select(t: Tab) {
    this.tab = t;
    try {
      localStorage.setItem(TAB_KEY, t);
    } catch {
      /* not remembered */
    }
    this.render();
    if (t === 'images') void this.loadAttachments();
  }

  private render() {
    this.renderOverview();
    this.renderFiles();
    this.renderPlan();
    this.renderImages();
    this.renderBadges();
    this.showTab();
  }

  private showTab() {
    const searching = this.query.length >= 2;
    for (const [id, p] of this.panels) p.hidden = searching || id !== this.tab;
    this.results.hidden = !searching;
    for (const [id, b] of this.tabBtns) {
      b.setAttribute('aria-selected', String(id === this.tab && !searching));
      b.classList.toggle('on', id === this.tab && !searching);
    }
  }

  private renderBadges() {
    const set = (t: Tab, text: string) => {
      const b = this.tabBadges.get(t)!;
      b.textContent = text;
      b.hidden = text === '';
    };
    const d = this.data;
    set('files', d && d.files.length > 0 ? String(d.files.length) : '');
    const open = d ? d.todos.filter((t) => t.status !== 'completed').length : 0;
    set('plan', open > 0 ? String(open) : d?.plan ? '•' : '');
    set('images', this.attachments.length > 0 ? String(this.attachments.length) : '');
  }

  // ------------------------------------------------------------------ overview

  private renderOverview() {
    const info = this.info;
    this.badge.hidden = !info?.agent;
    this.badge.textContent = info?.agent === 'claude' ? 'Claude' : info?.agent === 'codex' ? 'Codex' : '';
    this.badge.dataset.agent = info?.agent ?? '';

    const rows: HTMLElement[] = [];
    const a = this.actions;
    if (!info || !info.eligible || !a) {
      rows.push(el('div', 'rb-hint', 'Model, mode and conversation actions appear for Claude and Codex sessions.'));
    } else {
      rows.push(
        this.row('Model', info.model || 'Not reported yet', a.model),
        this.row('Mode', info.mode || 'Default', a.mode, info.yolo ? 'Permission checks skipped' : undefined),
        this.row('More actions', 'Reasoning, commands, edit last message', a.more),
        this.row('Resume', 'Continue an earlier conversation', a.resume),
      );
    }
    this.rows.replaceChildren(...rows);
  }

  private row(label: string, value: string, onClick: () => void, warn?: string): HTMLElement {
    const b = el('button', 'rb-row');
    b.type = 'button';
    const text = el('span', 'rb-row-text');
    text.append(el('span', 'rb-row-label', label), el('span', 'rb-row-value', value));
    if (warn) text.append(el('span', 'rb-row-warn', `⚡ ${warn}`));
    b.append(text, icon('chevron', 'icon-svg rb-chev'));
    b.addEventListener('click', onClick);
    return b;
  }

  // ------------------------------------------------------------------ files

  private renderFiles() {
    const p = this.panels.get('files')!;
    const files = this.data?.files ?? [];
    if (files.length === 0) {
      p.replaceChildren(el('div', 'rb-hint', 'Files the agent edits, writes or deletes in this conversation are listed here, with what changed.'));
      return;
    }
    const added = files.reduce((n, f) => n + f.added, 0);
    const removed = files.reduce((n, f) => n + f.removed, 0);
    const sum = el('div', 'rb-files-sum');
    sum.append(
      el('span', undefined, `${files.length} file${files.length === 1 ? '' : 's'}`),
      el('span', 'rb-add', `+${added}`),
      el('span', 'rb-del', `−${removed}`),
    );
    const list = el('div', 'rb-files');
    for (const f of files) {
      const d = el('details', 'rb-file');
      d.open = this.openFiles.has(f.path);
      d.addEventListener('toggle', () => {
        if (d.open) this.openFiles.add(f.path);
        else this.openFiles.delete(f.path);
      });
      const s = el('summary');
      const names = el('span', 'rb-file-names');
      names.append(el('span', 'rb-file-name', f.name));
      if (f.dir) names.append(el('span', 'rb-file-dir', f.dir));
      const tags: HTMLElement[] = [];
      if (f.created) tags.push(el('span', 'rb-file-tag new', 'new'));
      if (f.deleted) tags.push(el('span', 'rb-file-tag del', 'deleted'));
      if (f.pending) tags.push(el('span', 'rb-file-tag run', 'editing…'));
      if (f.failed > 0) tags.push(el('span', 'rb-file-tag err', `${f.failed} failed`));
      const delta = el('span', 'rb-file-delta');
      delta.append(el('span', 'rb-add', `+${f.added}`), el('span', 'rb-del', `−${f.removed}`));
      s.append(icon('chevron', 'icon-svg chev'), names, ...tags, delta);
      d.append(s);
      if (d.open) d.append(renderDiff({ path: f.path, edits: f.edits }));
      // Diffs are built when first opened, so a long list stays cheap.
      d.addEventListener('toggle', () => {
        if (d.open && d.childElementCount === 1) d.append(renderDiff({ path: f.path, edits: f.edits }));
      });
      list.append(d);
    }
    p.replaceChildren(sum, list);
  }

  // ------------------------------------------------------------------ plan

  private renderPlan() {
    const p = this.panels.get('plan')!;
    const todos = this.data?.todos ?? [];
    const plan = this.data?.plan ?? null;
    if (todos.length === 0 && !plan) {
      p.replaceChildren(
        el('div', 'rb-hint', 'The agent’s to-do list and any plan it proposes appear here. Claude’s plan mode and Codex’s plans both show up.'),
      );
      return;
    }
    const parts: HTMLElement[] = [];
    if (todos.length > 0) parts.push(this.renderTodos(todos));
    if (plan) {
      const d = el('details', 'rb-plan');
      d.open = true;
      d.append(el('summary', undefined, 'Plan'));
      d.append(renderMarkdown(plan));
      parts.push(d);
    }
    p.replaceChildren(...parts);
  }

  private renderTodos(todos: Todo[]): HTMLElement {
    const done = todos.filter((t) => t.status === 'completed').length;
    const box = el('section', 'rb-todos');
    const head = el('div', 'rb-todos-head');
    head.append(el('div', 'drawer-label', 'To-do'), el('span', 'rb-todos-count', `${done} of ${todos.length}`));
    const meter = el('div', 'meter');
    const fill = el('div', 'meter-fill');
    fill.style.width = `${(done / todos.length) * 100}%`;
    meter.append(fill);
    const list = el('ul', 'rb-todo-list');
    for (const t of todos) {
      const li = el('li', `rb-todo st-${t.status}`);
      const mark = el('span', 'rb-todo-mark');
      mark.textContent = t.status === 'completed' ? '✓' : t.status === 'in_progress' ? '●' : '';
      li.append(mark, el('span', 'rb-todo-text', t.status === 'in_progress' && t.active ? t.active : t.content));
      list.append(li);
    }
    box.append(head, meter, list);
    return box;
  }

  // ------------------------------------------------------------------ images

  private async loadAttachments() {
    if (!this.session) return;
    const name = this.session;
    try {
      const r = await fetch(`/api/sessions/${encodeURIComponent(name)}/attachments`);
      if (!r.ok || name !== this.session) return;
      const list = (await r.json()) as Attachment[];
      if (JSON.stringify(list) === JSON.stringify(this.attachments)) return;
      this.attachments = list;
      this.renderImages();
      this.renderBadges();
    } catch {
      /* offline: keep what is showing */
    }
  }

  private renderImages() {
    const p = this.panels.get('images')!;
    if (this.attachments.length === 0) {
      p.replaceChildren(el('div', 'rb-hint', 'Images you attach in the chat appear here. They are kept for a week.'));
      return;
    }
    const grid = el('div', 'rb-grid');
    for (const a of this.attachments) {
      const link = el('a', 'rb-thumb');
      link.href = a.url;
      link.target = '_blank';
      link.rel = 'noopener';
      link.setAttribute('aria-label', `Open ${a.name}`);
      const img = el('img');
      img.src = a.url;
      img.alt = a.name;
      img.loading = 'lazy';
      link.append(img);
      grid.append(link);
    }
    p.replaceChildren(grid);
  }

  // ------------------------------------------------------------------ search

  private setQuery(q: string) {
    this.query = q.trim();
    this.clear.hidden = this.search.value === '';
    this.runSearch();
    this.showTab();
  }

  private runSearch() {
    if (this.query.length < 2) {
      this.results.replaceChildren();
      return;
    }
    const hits = this.hooks.search(this.query);
    if (hits.length === 0) {
      this.results.replaceChildren(el('div', 'rb-hint', this.data || this.info?.eligible ? 'No messages match.' : 'Search works for Claude and Codex conversations.'));
      return;
    }
    const head = el('div', 'rb-results-head', `${hits.length}${hits.length >= 60 ? '+' : ''} match${hits.length === 1 ? '' : 'es'}`);
    const list = el('div', 'rb-hits');
    for (const h of hits) {
      const b = el('button', 'rb-hit');
      b.type = 'button';
      const snippet = el('span', 'rb-hit-text');
      snippet.append(document.createTextNode(h.before), el('mark', undefined, h.match), document.createTextNode(h.after));
      b.append(el('span', 'rb-hit-kind', KIND_LABEL[h.kind] ?? h.kind), snippet);
      b.addEventListener('click', () => this.hooks.jump(h.id));
      list.append(b);
    }
    this.results.replaceChildren(head, list);
  }
}
