// The left sidebar: your sessions grouped by project, a way to start new ones, rename, kill, switch.

import { fetchPresets, launchSession, mainAgents, type PresetInfo } from './dirs';
import { el } from './dom';
import { button } from './ui/button';
import { cn } from './ui/cn';
import { asTree, elapsedSecs, fmtElapsed, stateOf, type SubAgent } from './agents';
import { warn } from './haptics';
import { renderInline } from './chat-render';
import { icon } from './icons';
import { readSessionCache, writeSessionCache } from './sessions-cache';
import { previewInlines } from './markdown';
import { openMenu, type MenuItem } from './menu';
import { DOT_LABEL, dotState, groupSessions, PINNED_KEY, railLabel } from './session-groups';

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
  /** The working folder now. */
  cwd: string | null;
  /** The folder it was started in: the project it is listed under. */
  project?: string | null;
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

export interface SidebarHooks {
  /** A session was chosen or started: let an overlay sidebar get out of the way. */
  navigated: () => void;
  /** The header's hide button. */
  collapse: () => void;
  openSettings: () => void;
  /** Open the new-session sheet, with a project's folder filled in when there is one. */
  newSession: (dir?: string) => void;
  /** Project folders kept in the list even with no session in them. */
  pinnedProjects: () => string[];
  /** Keep a project in the list, or stop keeping it. */
  togglePinProject: (path: string) => Promise<void>;
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
  private msg = el('div', 'drawer-msg');
  private timer = 0;
  private active = false;
  /** A list from the server has been drawn: an offline fallback must not replace it. */
  private shown = false;
  private presetList: PresetInfo[] | null = null;
  /** Project sections the user folded away. Remembered. */
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

    const head = el('div', 'sb-head');
    const add = button({ variant: 'ghost', size: 'icon-sm', label: 'New session', cls: 'sb-new', onClick: () => this.hooks.newSession() }, icon('plus'));
    const collapse = button({ variant: 'ghost', size: 'icon-sm', label: 'Hide sessions', cls: 'sb-collapse', onClick: () => this.hooks.collapse() }, icon('panel-left'));
    head.append(el('strong', 'sb-brand', 'ttm'), el('span', 'sb-spacer'), add, collapse);

    const foot = el('div', 'sb-foot');
    const settings = button({ variant: 'ghost', cls: 'sb-settings w-full justify-start gap-2.5 text-muted-foreground', onClick: () => this.hooks.openSettings() }, icon('sliders'), 'Settings');
    foot.append(settings);

    // Split view shrinks the sidebar to this: one button per session, and the same way to Settings.
    const railOpen = button({ variant: 'ghost', size: 'icon-sm', label: 'Show the session list', cls: 'rail-expand', onClick: () => this.hooks.collapse() }, icon('panel-left'));
    this.railList.className = 'rail-list';
    const railSettings = button({ variant: 'ghost', size: 'icon-sm', label: 'Settings', cls: 'rail-settings', onClick: () => this.hooks.openSettings() }, icon('sliders'));
    this.rail.append(railOpen, this.railList, railSettings);

    this.root.append(head, this.msg, this.list, foot, this.rail);
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
      let r: Response;
      try {
        r = await api('/api/sessions');
      } catch (e) {
        if (e instanceof Error && e.message === 'unauthorized') throw e;
        // No answer. Rather than an empty sidebar, show the list as it last was, and say so.
        const cached = this.shown ? null : readSessionCache();
        if (cached) this.renderList(cached);
        return this.say(cached ? "Can't reach ttm. Showing your sessions as they were." : 'Server unreachable');
      }
      if (!r.ok) return this.say(await r.text());
      this.say('');
      const list = (await r.json()) as SessionView[];
      writeSessionCache(list);
      this.shown = true;
      this.renderList(list);
      for (const id of this.agentsOpen) void this.loadAgents(id);
    });
  }

  /** The presets a project's `+` offers. Fetched once; they only change with the config. */
  private async presets(): Promise<PresetInfo[]> {
    return (this.presetList ??= await fetchPresets());
  }

  /** Start Claude, Codex, a shell or another preset in a project's folder, and open it. */
  private async startIn(dir: string, preset?: PresetInfo) {
    await this.guard(async () => {
      const r = await launchSession({ preset: preset?.name, yolo: false, dir });
      if (!r.ok) {
        if (r.signedOut) throw new Error('unauthorized');
        return this.say(r.error);
      }
      this.hooks.navigated();
      this.open(r.id);
    });
  }

  /** The `+` beside a project: one tap for each of Claude, Codex and a shell there, and the full sheet. */
  private async projectMenu(anchor: HTMLElement, sec: { title: string; path: string }) {
    const presets = await this.presets();
    const { claude, codex, more } = mainAgents(presets);
    const items: MenuItem[] = [];
    if (claude) items.push({ label: 'Claude', hint: `in ${sec.title}`, run: () => void this.startIn(sec.path, claude) });
    if (codex) items.push({ label: 'Codex', hint: `in ${sec.title}`, run: () => void this.startIn(sec.path, codex) });
    items.push({ label: 'Shell', hint: `in ${sec.title}`, run: () => void this.startIn(sec.path) });
    for (const p of more) items.push({ label: p.name, hint: `in ${sec.title}`, run: () => void this.startIn(sec.path, p) });
    items.push({ label: 'More options…', hint: 'skip permissions, other folder', run: () => this.hooks.newSession(sec.path) });
    openMenu(anchor, items, `Start in ${sec.title}`);
  }

  private renderList(sessions: SessionView[]) {
    this.lastSessions = sessions;
    this.renderRail(sessions);
    const sections = groupSessions(sessions, this.hooks.pinnedProjects());
    if (sections.length === 0) {
      const start = button({ variant: 'secondary', cls: 'drawer-start w-full', onClick: () => this.hooks.newSession() }, icon('plus'), 'New session');
      this.list.replaceChildren(el('div', 'drawer-empty', 'No sessions yet'), start);
      return;
    }
    // Sessions with no project at all need no heading: it would only say "Other".
    if (sections.length === 1 && sections[0]!.path === null && sections[0]!.key !== PINNED_KEY) {
      this.list.replaceChildren(...sections[0]!.items.map((s) => this.row(s)));
      return;
    }
    const out: HTMLElement[] = [];
    for (const sec of sections) {
      const folded = this.collapsed.has(sec.key);
      const head = el('div', 'sg-head');
      const fold = el('button', 'sg-fold');
      fold.type = 'button';
      fold.setAttribute('aria-expanded', String(!folded));
      const waiting = sec.items.some((s) => dotState(s) === 'waiting');
      fold.append(icon('chevron', `icon-svg chev${folded ? '' : ' open'}`), icon(sec.key === PINNED_KEY ? 'panel-left' : 'folder'));
      fold.append(el('span', 'sg-title', sec.title));
      if (sec.sub) fold.append(el('span', 'sg-sub', sec.sub));
      fold.append(el('span', 'sg-spacer'));
      // A fold hides the rows, so say if something behind it needs you.
      if (folded && waiting) fold.append(el('span', 'dot waiting'));
      if (sec.items.length > 0) fold.append(el('span', 'sg-count', String(sec.items.length)));
      fold.addEventListener('click', () => {
        if (folded) this.collapsed.delete(sec.key);
        else this.collapsed.add(sec.key);
        try {
          localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...this.collapsed]));
        } catch {
          /* not remembered */
        }
        this.renderList(this.lastSessions);
      });
      head.append(fold);
      const path = sec.path;
      if (path) {
        const branch = this.branchOf(sec.items);
        if (branch) head.append(el('span', 'sg-branch', branch));
        const act = 'sg-act size-ctl-sm text-muted-foreground aria-expanded:bg-muted aria-expanded:text-foreground';
        const pin = button(
          { variant: 'ghost', size: 'icon-sm', label: sec.kept ? `Stop keeping ${sec.title} in the list` : `Keep ${sec.title} in the list`, cls: cn(act, sec.kept && 'text-warn'),
            onClick: () => void this.hooks.togglePinProject(path).then(() => this.renderList(this.lastSessions)) },
          sec.kept ? '★' : '☆',
        );
        pin.title = sec.kept ? 'Kept in the list' : 'Keep in the list when nothing runs in it';
        const add = button({ variant: 'ghost', size: 'icon-sm', label: `Start a session in ${sec.title}`, cls: act, onClick: () => void this.projectMenu(add, { title: sec.title, path }) }, icon('plus'));
        add.setAttribute('aria-haspopup', 'menu');
        add.setAttribute('aria-expanded', 'false');
        head.append(pin, add);
      }
      out.push(head);
      if (!folded) out.push(...sec.items.map((s) => this.row(s)));
    }
    this.list.replaceChildren(...out);
  }

  /** The branch most of a project's running sessions are on, for its heading. */
  private branchOf(items: SessionView[]): string | null {
    return items.find((s) => s.status === 'running' && s.branch)?.branch ?? null;
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
    const kind = el('span', `srow-kind${s.agent === 'claude' || s.agent === 'codex' ? ` ${s.agent}` : ''}`);
    kind.title = s.agent && s.agent !== 'other' ? s.agent : s.preset ?? 'shell';
    kind.append(icon(s.agent ? 'agent' : 'shell'));
    name.append(kind, document.createTextNode(`${s.pinned ? '★ ' : ''}${s.id}`));
    if (s.yolo) {
      const bolt = el('span', 'srow-yolo');
      bolt.append(icon('shield-alert', 'size-3.5'));
      bolt.title = 'Started with permission checks skipped';
      bolt.setAttribute('aria-label', 'Permission checks skipped');
      name.append(bolt);
    }
    if (state === 'waiting') name.append(el('span', 'srow-flag', 'Needs you'));
    main.append(name);
    if (s.preview) {
      // Inside a single child so the two-line clamp counts lines of the whole run, not of each element.
      const text = el('span');
      text.append(...renderInline(previewInlines(s.preview)));
      const preview = el('span', 'srow-preview');
      preview.append(text);
      main.append(preview);
    }
    // Where the shell has wandered to since the session started, when that is not the project itself.
    const drift = this.drift(s);
    if (drift) {
      const where = el('span', 'srow-where');
      where.append(icon('folder'), el('span', undefined, drift));
      main.append(where);
    }
    const bits: string[] = [];
    if (s.status === 'stopped') bits.push(s.resumable ? 'stopped · can resume' : 'stopped');
    else if (s.status === 'exited') bits.push(`exited${s.exit_code !== null ? ` (${s.exit_code})` : ''}`);
    else bits.push(state === 'working' ? 'working' : `idle ${ago(s.idle_secs)}`);
    if (s.routed_via === 'teamclaude') bits.push('via Team Claude');
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
    if (s.status !== 'running') {
      inline.push(button({ size: 'sm', onClick: () => void this.restore(s.id, false) }, s.resumable ? 'Resume' : 'Restore'));
    }

    // Pin, split, rename and remove live in one menu: the `⋯`, or a long press on the row.
    const more = button({ variant: 'ghost', size: 'icon-sm', label: `Actions for ${s.id}`, cls: 'srow-more h-11 text-muted-foreground aria-expanded:bg-muted aria-expanded:text-foreground' }, icon('more'));
    more.setAttribute('aria-haspopup', 'menu');
    more.setAttribute('aria-expanded', 'false');
    const menu = () => openMenu(more, this.rowMenu(s), `Actions for ${s.id}`);
    more.addEventListener('click', menu);
    this.onLongPress(main, menu);

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

    row.append(dot, main, ...inline, more);
    if (open) row.append(this.agentList(s));
    return row;
  }

  /** Where a session's shell is now, relative to its project, when it is somewhere else. */
  private drift(s: SessionView): string | null {
    if (!s.project || !s.cwd || s.cwd === s.project) return null;
    return s.cwd.startsWith(`${s.project}/`) ? s.cwd.slice(s.project.length + 1) : s.cwd;
  }

  private rowMenu(s: SessionView): MenuItem[] {
    const items: MenuItem[] = [
      { label: s.pinned ? 'Unpin' : 'Pin to top', run: () => void this.patchMeta(s.id, { pinned: !s.pinned }) },
      {
        label: 'Open in a split',
        hint: 'beside the current session',
        run: () => {
          this.hooks.openInSplit(s.id);
          this.hooks.navigated();
        },
      },
      { label: 'Rename, note, tags…', run: () => void this.edit(s) },
      { label: s.status === 'stopped' ? 'Remove' : 'Kill', danger: true, run: () => void this.kill(s) },
    ];
    return items;
  }

  /** Call `fn` when the pointer stays down on `target` for a moment without moving (a touch long-press). */
  private onLongPress(target: HTMLElement, fn: () => void) {
    let timer = 0;
    let x = 0;
    let y = 0;
    let fired = false;
    let mouse = false;
    const cancel = () => window.clearTimeout(timer);
    target.addEventListener('pointerdown', (e) => {
      mouse = e.pointerType === 'mouse';
      if (mouse) return;
      fired = false;
      x = e.clientX;
      y = e.clientY;
      timer = window.setTimeout(() => {
        fired = true;
        fn();
      }, 500);
    });
    target.addEventListener('pointermove', (e) => {
      if (Math.abs(e.clientX - x) > 8 || Math.abs(e.clientY - y) > 8) cancel();
    });
    for (const ev of ['pointerup', 'pointercancel', 'pointerleave'] as const) target.addEventListener(ev, cancel);
    // The press that opened the menu must not also open the session when the finger lifts.
    target.addEventListener(
      'click',
      (e) => {
        if (!fired) return;
        fired = false;
        e.stopImmediatePropagation();
        e.preventDefault();
      },
      true,
    );
    // A finger's long press also raises this event: the timer above has it covered, and the system's own menu is unwanted.
    // A mouse's right-click is the same menu, on demand.
    target.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      if (mouse) fn();
    });
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
