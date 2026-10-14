// The left sidebar: list sessions, create from presets, rename, kill, switch.

import { el } from './dom';
import { asTree, elapsedSecs, fmtElapsed, stateOf, type SubAgent } from './agents';
import { warn } from './haptics';
import { icon } from './icons';
import { DOT_LABEL, dotState, groupSessions, railLabel } from './session-groups';

export interface SessionView {
  id: string;
  preset: string | null;
  status: 'running' | 'exited' | 'stopped';
  exit_code: number | null;
  idle_secs: number;
  attached: number;
  created: number;
  command: string;
  restarts: number;
  restart_in_secs: number | null;
  agent: 'claude' | 'codex' | 'other' | null;
  resumable: boolean;
  has_agent_session: boolean;
  group: string | null;
  note: string | null;
  tags: string[];
  pinned: boolean;
  /** Started with permission checks skipped. */
  yolo: boolean;
  /** The working folder. */
  cwd: string | null;
  /** The git branch checked out there. */
  branch: string | null;
  /** What a running Claude or Codex session is doing right now. */
  activity: 'working' | 'waiting' | 'idle' | 'unknown' | null;
  /** One line of the latest message. */
  preview: string | null;
  /** How many helper agents the session has running. */
  running_subagents?: number;
  /** The proxy the session's agent goes through, when it does. */
  routed_via?: 'teamclaude' | null;
}

interface Group {
  name: string;
  presets: string[];
}

interface Preset {
  name: string;
  command: string;
  restart: 'never' | 'on_failure';
  /** Only Claude and Codex presets can skip permission checks. */
  agent?: 'claude' | 'codex' | 'other';
  /** Always started with permissions skipped (set in the config). */
  yolo?: boolean;
}

const canSkip = (p: Preset) => p.agent === 'claude' || p.agent === 'codex';

const POLL_MS = 3000;
const ACTIVE_SECS = 5;
const COLLAPSED_KEY = 'ttm.groups.collapsed';

function loadCollapsed(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '[]') as unknown;
    return new Set(Array.isArray(raw) ? raw.filter((k): k is string => typeof k === 'string') : []);
  } catch {
    return new Set();
  }
}

function ago(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86400)}d`;
}

async function api(path: string, init?: RequestInit): Promise<Response> {
  const r = await fetch(path, {
    ...init,
    headers: { 'content-type': 'application/json', ...(init?.headers ?? {}) },
  });
  if (r.status === 401) throw new Error('unauthorized');
  return r;
}

/** A folder you have been working in, from `/api/dirs`. */
interface RecentDir {
  path: string;
  name: string;
  short: string;
}

export interface SidebarHooks {
  /** A session was chosen or started: let an overlay sidebar get out of the way. */
  navigated: () => void;
  /** The header's hide button. */
  collapse: () => void;
  openSettings: () => void;
  /** Show a session in the pane beside the one you are in (wide screens). */
  openInSplit: (id: string) => void;
  /** Open a helper agent's transcript. */
  openAgent: (session: string, agent: SubAgent) => void;
  /** Every session open in a pane, to mark in the list. */
  shown: () => string[];
  /** Progress a program reported (OSC 9;4) in a session open in a pane, if any. */
  progress: (id: string) => string | null;
  /** A pane you were not looking at rang its bell. */
  attention: (id: string) => boolean;
}

/** The left sidebar: your sessions, a way to start new ones, and the way into Settings. */
export class SessionSidebar {
  private list = el('div', 'drawer-list');
  /** The narrow strip of session buttons shown instead of the list in split view. */
  private rail = el('div', 'sb-rail');
  private railList = el('div');
  private presets = el('div', 'drawer-presets');
  private msg = el('div', 'drawer-msg');
  private timer = 0;
  private active = false;
  private presetsLoaded = false;
  /** The "skip permissions" switch. Off to start with, and off again after every launch. */
  private skip = false;
  private presetList: Preset[] = [];
  private dirs: RecentDir[] = [];
  private groupList: Group[] = [];
  /** Rows whose pin / edit / remove buttons are showing. Kept across the list's refreshes. */
  private expanded = new Set<string>();
  /** Folder sections the user folded away. Remembered. */
  private collapsed = loadCollapsed();
  private lastSessions: SessionView[] = [];
  /** Sessions whose helper agents are listed under their row, and the latest lists fetched. */
  private agentsOpen = new Set<string>();
  private agentCache = new Map<string, SubAgent[]>();

  constructor(
    private root: HTMLElement,
    private current: () => string,
    private open: (id: string) => void,
    private onUnauthorized: () => void,
    private hooks: SidebarHooks,
    /** What a session has used so far, e.g. "12k in · 4.1k out", when known. */
    private tokens: (id: string) => string | undefined = () => undefined,
  ) {
    this.root.classList.add('sidebar', 'sidebar-left');
    this.root.setAttribute('aria-label', 'Sessions');
    this.presets.hidden = true;

    const head = el('div', 'sb-head');
    const add = el('button', 'icon sb-new');
    add.type = 'button';
    add.setAttribute('aria-label', 'New session');
    add.append(icon('plus'));
    add.addEventListener('click', () => (this.presets.hidden = !this.presets.hidden));
    const collapse = el('button', 'icon sb-collapse');
    collapse.type = 'button';
    collapse.setAttribute('aria-label', 'Hide sessions');
    collapse.append(icon('panel-left'));
    collapse.addEventListener('click', () => this.hooks.collapse());
    head.append(el('strong', 'sb-brand', 'ttm'), el('span', 'sb-spacer'), add, collapse);

    const foot = el('div', 'sb-foot');
    const settings = el('button', 'sb-settings');
    settings.type = 'button';
    settings.append(icon('sliders'), el('span', undefined, 'Settings'));
    settings.addEventListener('click', () => this.hooks.openSettings());
    foot.append(settings);

    // Split view shrinks the sidebar to this: one button per session, and the same way to Settings.
    const railOpen = el('button', 'icon rail-expand');
    railOpen.type = 'button';
    railOpen.setAttribute('aria-label', 'Show the session list');
    railOpen.append(icon('panel-left'));
    railOpen.addEventListener('click', () => this.hooks.collapse());
    this.railList.className = 'rail-list';
    const railSettings = el('button', 'icon rail-settings');
    railSettings.type = 'button';
    railSettings.setAttribute('aria-label', 'Settings');
    railSettings.append(icon('sliders'));
    railSettings.addEventListener('click', () => this.hooks.openSettings());
    this.rail.append(railOpen, this.railList, railSettings);

    this.root.append(head, this.presets, this.msg, this.list, foot, this.rail);
  }

  /** Poll the session list while the sidebar is on screen. */
  setActive(on: boolean) {
    if (on === this.active) return;
    this.active = on;
    window.clearInterval(this.timer);
    if (on) {
      void this.refresh();
      this.timer = window.setInterval(() => void this.refresh(), POLL_MS);
    }
  }

  /** Refresh now, e.g. after switching session so the highlight moves. Not while the list is off screen. */
  reload() {
    if (this.active) void this.refresh();
  }

  private say(text: string) {
    this.msg.textContent = text;
  }

  private async guard<T>(fn: () => Promise<T>): Promise<T | undefined> {
    try {
      return await fn();
    } catch (e) {
      if (e instanceof Error && e.message === 'unauthorized') this.onUnauthorized();
      else this.say('Server unreachable');
      return undefined;
    }
  }

  private async refresh() {
    await this.guard(async () => {
      if (!this.presetsLoaded) {
        const [p, g] = await Promise.all([api('/api/presets'), api('/api/groups')]);
        if (p.ok) {
          this.renderPresets((await p.json()) as Preset[], g.ok ? ((await g.json()) as Group[]) : []);
          this.presetsLoaded = true;
        }
      }
      const d = await api('/api/dirs');
      if (d.ok) {
        const dirs = (await d.json()) as RecentDir[];
        if (JSON.stringify(dirs) !== JSON.stringify(this.dirs)) {
          this.dirs = dirs;
          this.renderPresets(this.presetList, this.groupList);
        }
      }
      const r = await api('/api/sessions');
      if (!r.ok) return this.say(await r.text());
      this.say('');
      this.renderList((await r.json()) as SessionView[]);
      for (const id of this.agentsOpen) void this.loadAgents(id);
    });
  }

  private renderPresets(presets: Preset[], groups: Group[]) {
    this.presetList = presets;
    this.groupList = groups;
    this.presets.replaceChildren(el('div', 'drawer-label', 'New session'));
    // Only offered when there is a Claude or Codex preset to apply it to.
    if (presets.some(canSkip)) {
      const t = el('button', this.skip ? 'skip-toggle on' : 'skip-toggle');
      t.type = 'button';
      t.setAttribute('aria-pressed', String(this.skip));
      t.append(el('span', 'skip-dot'), el('span', 'skip-text', 'Skip permissions'));
      t.title = 'Claude: IS_SANDBOX=1 and --dangerously-skip-permissions. Codex: --yolo. Applies to the next session you start.';
      t.addEventListener('click', () => {
        this.skip = !this.skip;
        this.renderPresets(this.presetList, this.groupList);
      });
      this.presets.append(t);
    }
    const row = el('div', 'drawer-chips');
    for (const p of [{ name: 'shell' } as Preset, ...presets]) {
      const risky = canSkip(p) && (this.skip || p.yolo === true);
      const b = el('button', risky ? 'chip chip-yolo' : 'chip', risky ? `⚡ ${p.name}` : p.name);
      b.type = 'button';
      if (p.command) b.title = p.command;
      b.addEventListener('click', () => void this.create(p.command ? p.name : undefined, canSkip(p) && this.skip));
      row.append(b);
    }
    for (const g of groups) {
      const b = el('button', 'chip chip-group', `▶ ${g.name}`);
      b.type = 'button';
      b.title = `Start together: ${g.presets.join(', ')}`;
      b.addEventListener('click', () => void this.startGroup(g.name));
      row.append(b);
    }
    this.presets.append(row);
    this.renderDirs(presets.filter(canSkip));
  }

  /** Folders you have used in tmux, each with a button per agent to start it there. */
  private renderDirs(agents: Preset[]) {
    if (this.dirs.length === 0 || agents.length === 0) return;
    this.presets.append(el('div', 'drawer-label', 'Start in a folder'));
    const list = el('div', 'dir-list');
    for (const d of this.dirs) {
      const item = el('div', 'dir-row');
      const text = el('div', 'dir-text');
      text.append(el('span', 'dir-name', d.name), el('span', 'dir-path', d.short));
      text.title = d.path;
      const btns = el('div', 'dir-btns');
      for (const p of agents) {
        const risky = this.skip || p.yolo === true;
        const b = el('button', risky ? 'chip chip-yolo' : 'chip', risky ? `⚡ ${p.name}` : p.name);
        b.type = 'button';
        b.title = `${p.name} in ${d.short}`;
        b.addEventListener('click', () => void this.create(p.name, this.skip, d.path));
        btns.append(b);
      }
      item.append(text, btns);
      list.append(item);
    }
    this.presets.append(list);
  }

  private async startGroup(name: string) {
    await this.guard(async () => {
      const r = await api(`/api/groups/${encodeURIComponent(name)}/start`, { method: 'POST' });
      if (!r.ok) return this.say(await r.text());
      const { ids } = (await r.json()) as { ids: string[] };
      this.hooks.navigated();
      if (ids[0]) this.open(ids[0]);
    });
  }

  private async create(preset?: string, yolo = false, dir?: string) {
    // One launch at a time: the switch goes back off whatever happens.
    this.skip = false;
    this.presets.hidden = true;
    this.renderPresets(this.presetList, this.groupList);
    await this.guard(async () => {
      const r = await api('/api/sessions', { method: 'POST', body: JSON.stringify({ preset, yolo, dir }) });
      if (!r.ok) return this.say(await r.text());
      const { id } = (await r.json()) as { id: string };
      this.hooks.navigated();
      this.open(id);
    });
  }

  private renderList(sessions: SessionView[]) {
    this.lastSessions = sessions;
    this.renderRail(sessions);
    if (sessions.length === 0) {
      this.list.replaceChildren(el('div', 'drawer-empty', 'No sessions yet'));
      this.presets.hidden = false;
      return;
    }
    const sections = groupSessions(sessions);
    // One folder (or none) needs no headings: they would only repeat what every row says.
    if (sections.length < 2) {
      this.list.replaceChildren(...sessions.map((s) => this.row(s)));
      return;
    }
    const out: HTMLElement[] = [];
    for (const sec of sections) {
      const folded = this.collapsed.has(sec.key);
      const head = el('button', 'sg-head');
      head.type = 'button';
      head.setAttribute('aria-expanded', String(!folded));
      const waiting = sec.items.some((s) => dotState(s) === 'waiting');
      head.append(icon('chevron', `icon-svg chev${folded ? '' : ' open'}`), icon(sec.key === '__pinned' ? 'panel-left' : 'folder'));
      head.append(el('span', 'sg-title', sec.title));
      if (sec.sub) head.append(el('span', 'sg-sub', sec.sub));
      head.append(el('span', 'sg-spacer'));
      // A fold hides the rows, so say if something behind it needs you.
      if (folded && waiting) head.append(el('span', 'dot waiting'));
      head.append(el('span', 'sg-count', String(sec.items.length)));
      head.addEventListener('click', () => {
        if (folded) this.collapsed.delete(sec.key);
        else this.collapsed.add(sec.key);
        try {
          localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...this.collapsed]));
        } catch {
          /* not remembered */
        }
        this.renderList(this.lastSessions);
      });
      out.push(head);
      if (!folded) out.push(...sec.items.map((s) => this.row(s)));
    }
    this.list.replaceChildren(...out);
  }

  /** Fetch a session's helper agents, and redraw the list if they changed. */
  private async loadAgents(id: string) {
    await this.guard(async () => {
      const r = await api(`/api/sessions/${encodeURIComponent(id)}/agents`);
      if (!r.ok) return;
      const list = (await r.json()) as SubAgent[];
      if (JSON.stringify(list) === JSON.stringify(this.agentCache.get(id))) return;
      this.agentCache.set(id, list);
      this.renderList(this.lastSessions);
    });
  }

  /** The helper agents of one session as a tree, under its row. */
  private agentList(s: SessionView): HTMLElement {
    const box = el('div', 'srow-agentlist');
    const list = this.agentCache.get(s.id);
    if (!list) {
      box.append(el('div', 'sa-empty', 'Loading…'));
      return box;
    }
    if (list.length === 0) box.append(el('div', 'sa-empty', 'No helper agents yet'));
    const now = Math.floor(Date.now() / 1000);
    for (const { agent: a, level } of asTree(list)) {
      const state = stateOf(a);
      const b = el('button', 'sa-row');
      b.type = 'button';
      b.style.setProperty('--lvl', String(level));
      const secs = elapsedSecs(a, now);
      b.append(
        el('span', `dot ${state === 'done' ? 'ready' : state === 'running' ? 'working' : 'idle'}`),
        el('span', 'sa-kind', a.kind === 'codex' ? 'Codex' : a.kind),
        el('span', 'sa-desc', a.description || 'helper'),
        el('span', 'sa-time', secs === null ? '' : fmtElapsed(secs)),
      );
      b.disabled = !a.has_log;
      b.addEventListener('click', () => {
        this.hooks.openAgent(s.id, a);
        this.hooks.navigated();
      });
      box.append(b);
    }
    return box;
  }

  private renderRail(sessions: SessionView[]) {
    const shown = this.hooks.shown();
    this.railList.replaceChildren(
      ...sessions.map((s) => {
        const state = dotState(s, ACTIVE_SECS);
        const b = el('button', `rail-btn${shown.includes(s.id) ? ' on' : ''}`);
        b.type = 'button';
        b.title = `${s.id}: ${DOT_LABEL[state]}`;
        b.setAttribute('aria-label', `${s.id}, ${DOT_LABEL[state]}`);
        b.append(el('span', 'rail-letter', railLabel(s.id)), el('span', `dot ${state}`));
        b.addEventListener('click', () => this.open(s.id));
        return b;
      }),
    );
  }

  private row(s: SessionView): HTMLElement {
    const row = el('div', 'srow');
    if (this.hooks.shown().includes(s.id)) row.classList.add('current');

    const state = dotState(s, ACTIVE_SECS);
    if (state === 'waiting') row.classList.add('waiting');
    if (this.hooks.attention(s.id)) row.classList.add('bell');
    const dot = el('span', `dot ${state}`);
    dot.setAttribute('aria-label', DOT_LABEL[state]);
    const prog = this.hooks.progress(s.id);
    if (prog) dot.dataset.progress = prog;

    const main = el('button', 'srow-main');
    main.type = 'button';
    const name = el('span', 'srow-name');
    name.append(document.createTextNode(`${s.pinned ? '★ ' : ''}${s.id}`));
    if (state === 'waiting') name.append(el('span', 'srow-flag', 'Needs you'));
    main.append(name);
    if (s.preview) main.append(el('span', 'srow-preview', s.preview));
    if (s.branch) {
      const where = el('span', 'srow-where');
      where.append(icon('branch'), el('span', undefined, s.branch));
      main.append(where);
    }
    const bits: string[] = [];
    if (s.agent) bits.push(s.agent);
    else if (s.preset) bits.push(s.preset);
    if (s.status === 'stopped') bits.push(s.resumable ? 'stopped · can resume' : 'stopped');
    else if (s.status === 'exited') bits.push(`exited${s.exit_code !== null ? ` (${s.exit_code})` : ''}`);
    else bits.push(state === 'working' ? 'working' : `idle ${ago(s.idle_secs)}`);
    if (s.routed_via === 'teamclaude') bits.push('via Team Claude');
    if (s.yolo) bits.push('⚡ permissions skipped');
    if (s.attached > 0) bits.push(`${s.attached} attached`);
    if (s.restarts > 0) bits.push(`${s.restarts}× restarted`);
    if (s.restart_in_secs !== null) bits.push(`restarting in ${s.restart_in_secs}s`);
    if (s.group) bits.push(`group ${s.group}`);
    main.append(el('span', 'srow-meta', bits.join(' · ')));
    const used = this.tokens(s.id);
    if (used) main.append(el('span', 'srow-meta', used));
    if (s.note) main.append(el('span', 'srow-note', s.note));
    if (s.tags.length > 0) {
      const tags = el('span', 'srow-tags');
      for (const t of s.tags) tags.append(el('span', 'tag', t));
      main.append(tags);
    }
    main.addEventListener('click', () => {
      this.hooks.navigated();
      this.open(s.id);
    });

    const inline: HTMLElement[] = [];
    const tools: HTMLElement[] = [];
    if (s.status !== 'running') {
      const label = s.resumable ? 'Resume' : 'Restore';
      const b = el('button', 'chip chip-go', label);
      b.type = 'button';
      b.addEventListener('click', () => void this.restore(s.id, false));
      inline.push(b);
    }
    const pin = el('button', 'icon', s.pinned ? '★' : '☆');
    pin.type = 'button';
    pin.setAttribute('aria-label', s.pinned ? `Unpin ${s.id}` : `Pin ${s.id}`);
    pin.addEventListener('click', () => void this.patchMeta(s.id, { pinned: !s.pinned }));
    tools.push(pin);

    const split = el('button', 'icon srow-split');
    split.type = 'button';
    split.setAttribute('aria-label', `Open ${s.id} in a split`);
    split.title = 'Open beside the current session';
    split.append(icon('columns'));
    split.addEventListener('click', () => {
      this.hooks.openInSplit(s.id);
      this.hooks.navigated();
    });
    tools.push(split);

    const edit = el('button', 'icon', '✎');
    edit.type = 'button';
    edit.setAttribute('aria-label', `Edit ${s.id}`);
    edit.addEventListener('click', () => void this.edit(s));
    tools.push(edit);

    const kill = el('button', 'icon danger', '✕');
    kill.type = 'button';
    kill.setAttribute('aria-label', `Remove ${s.id}`);
    kill.addEventListener('click', () => void this.kill(s));
    tools.push(kill);

    const more = el('button', 'icon srow-more', '⋯');
    more.type = 'button';
    more.setAttribute('aria-label', `Actions for ${s.id}`);
    more.setAttribute('aria-expanded', String(this.expanded.has(s.id)));
    more.addEventListener('click', () => {
      const open = !this.expanded.has(s.id);
      if (open) this.expanded.add(s.id);
      else this.expanded.delete(s.id);
      row.classList.toggle('expanded', open);
      more.setAttribute('aria-expanded', String(open));
    });
    const toolRow = el('div', 'srow-tools');
    toolRow.append(...tools);
    if (this.expanded.has(s.id)) row.classList.add('expanded');

    // Helper agents: a count while any run, and a tap to list them under the row.
    const running = s.running_subagents ?? 0;
    const open = this.agentsOpen.has(s.id);
    if (running > 0 || open) {
      const chip = el('button', 'srow-agents');
      chip.type = 'button';
      chip.setAttribute('aria-expanded', String(open));
      chip.setAttribute('aria-label', running > 0 ? `${running} helper agent${running === 1 ? '' : 's'} running` : 'Helper agents');
      chip.append(icon('agent'), el('span', undefined, running > 0 ? String(running) : ''));
      chip.addEventListener('click', () => {
        if (this.agentsOpen.has(s.id)) this.agentsOpen.delete(s.id);
        else {
          this.agentsOpen.add(s.id);
          void this.loadAgents(s.id);
        }
        this.renderList(this.lastSessions);
      });
      inline.push(chip);
    }

    row.append(dot, main, ...inline, more, toolRow);
    if (open) row.append(this.agentList(s));
    return row;
  }

  private async restore(id: string, fresh: boolean) {
    await this.guard(async () => {
      const r = await api(`/api/sessions/${encodeURIComponent(id)}/restore`, {
        method: 'POST',
        body: JSON.stringify({ fresh }),
      });
      if (!r.ok) return this.say(await r.text());
      this.hooks.navigated();
      this.open(id);
    });
  }

  private async patchMeta(id: string, body: { note?: string; tags?: string[]; pinned?: boolean }) {
    await this.guard(async () => {
      const r = await api(`/api/sessions/${encodeURIComponent(id)}/meta`, { method: 'POST', body: JSON.stringify(body) });
      if (!r.ok) return this.say(await r.text());
      await this.refresh();
    });
  }

  /** Name, note and tags through simple prompts (works well on phones). Cancel stops at any step. */
  private async edit(s: SessionView) {
    const name = window.prompt('Name:', s.id);
    if (name === null) return;
    const note = window.prompt(`Note for "${s.id}" (empty clears it):`, s.note ?? '');
    if (note === null) return;
    const tags = window.prompt('Tags, comma separated:', s.tags.join(', '));
    if (tags === null) return;
    await this.patchMeta(s.id, { note, tags: tags.split(',').map((t) => t.trim()).filter(Boolean) });
    if (name.trim() && name.trim() !== s.id) await this.renameTo(s.id, name.trim());
  }

  private async renameTo(id: string, name: string) {
    await this.guard(async () => {
      const r = await api(`/api/sessions/${encodeURIComponent(id)}/rename`, {
        method: 'POST',
        body: JSON.stringify({ name }),
      });
      if (!r.ok) return this.say(await r.text());
      if (id === this.current()) this.open(name);
      await this.refresh();
    });
  }

  private async kill(s: SessionView) {
    const msg =
      s.status === 'stopped'
        ? `Remove "${s.id}" and its saved output? It can't be restored afterwards.`
        : `Kill session "${s.id}"? The process inside will be terminated and it can't be restored.`;
    if (!window.confirm(msg)) return;
    warn();
    await this.guard(async () => {
      const r = await api(`/api/sessions/${encodeURIComponent(s.id)}`, { method: 'DELETE' });
      if (!r.ok) return this.say(await r.text());
      await this.refresh();
    });
  }
}
