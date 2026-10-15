import '@fontsource-variable/inter/wght.css';
import '@fontsource-variable/jetbrains-mono/wght.css';
import './ui/tailwind.css';
import { initTheme, setAgentAccent } from './theme';
import { initUiScale } from './scale';
import { SessionSidebar, type SessionView } from './sessions';
import { UsageView } from './usage';
import { Layout } from './layout';
import { initSidebarResize } from './sidebar-resize';
import { edgeSwipe } from './gestures';
import { RightBar } from './rightbar';
import { armBell } from './bell';
import { CommandBar } from './command-bar';
import { remember } from './command-model';
import { rootStep, type Ctx } from './commands';
import { SettingsSheet } from './settings';
import { Pane, type PaneEvents } from './pane';
import { el } from './dom';
import { FileViewer } from './file-viewer';
import { LayoutSheet } from './layout-sheet';
import {
  corners, dividers, dragSizes, effectivePreset, isPresetId, neighbor, preset, presetClass, presetForCount, sanitizeSizes, span,
  tileCount, tileNames, usable, visiblePanes, type Dir, type Divider, type Preset, type PresetId,
} from './tiling';
import { defaultLayouts, fetchLayouts, MAX_PANES_LIMIT, paneLimit, updateLayouts, type Layouts, type Workspace } from './workspace';
import { AgentSheet } from './agent-sheet';
import { fetchPresets, launchSession, loadNewSessionData, mainAgents, type PresetInfo } from './dirs';
import { openMenu, type MenuItem } from './menu';
import { isOverlayTraversal, onAddressRestore, pushOverlay } from './overlays';
import { watchForUpdates } from './update';
import { readSessionCache } from './sessions-cache';
import { parseLaunch, sameLaunch, startSession, type Launch } from './launch-model';
import { NewSessionSheet } from './new-session-sheet';
import { chip } from './ui/chip';
import { isOrchHash, orchOpen, syncOrchRoute } from './orch/entry';
import { SHEET_SELECTOR } from './ui/sheet';
import type { SubAgent } from './agents';

// Start the web fonts now rather than when the first text uses them: no swap flash, and the terminal measures its
// cells with the real font the first time instead of fitting twice.
void Promise.all([document.fonts?.load("1rem 'Inter Variable'"), document.fonts?.load("14px 'JetBrains Mono Variable'")]).catch(() => {});

initTheme();
initUiScale();
armBell();

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const layout = new Layout($('app'));
initSidebarResize($('app'));
let sidebar: SessionSidebar | null = null;
let rightbar: RightBar | null = null;
let settings: SettingsSheet | null = null;
let usage: UsageView | null = null;

/**
 * The open panes, in tile order. There can be more open than the screen has room for: the extra
 * ones stay alive behind a strip of tabs.
 */
const panes: Pane[] = [];
let focused: Pane | null = null;
/** The settings allow several panes and the screen has room for at least two tiles. */
let canSplit = false;
/** In tiled view the session list is an icon rail; this widens it back to the full list. */
let railOpen = false;

const TILES_KEY = 'ttm.tiles';
const LAST_KEY = 'ttm.last';
const OLD_SPLIT_KEY = 'ttm.split';
const RAIL_W = 64;
/** Browsers allow only a few WebGL contexts (fewer on a tablet): the busiest panes get one. */
const GPU_BUDGET = 4;

interface Sizes {
  cols: number[];
  rows: number[];
}

let layouts: Layouts = defaultLayouts();
/** The grid asked for; the one drawn is the biggest of those that fit (`current`). */
let asked: PresetId = 'single';
/** The grid was picked by hand, so it is kept even when panes close, instead of following the pane count. */
let explicit = false;
let current: Preset = preset('single');
/** The pane in each tile of `current`, or null for an empty tile. */
let slotOf: (Pane | null)[] = [];
const sizes = new Map<PresetId, Sizes>();
const lastFocus = new Map<Pane, number>();
let focusTick = 0;
/** How many panes there were the last time the tiles were laid out. */
let lastCount = 0;

const tilesEl = el('div', 'tiles');
const tabsEl = el('div', 'tile-tabs');
tabsEl.hidden = true;
$('panes').append(tabsEl, tilesEl);

const limit = () => paneLimit(layouts.settings);
const defaultPreset = (): PresetId => layouts.settings[presetClass(window.innerWidth)];

function sessionName(): string {
  if (isOrchHash()) return 'main'; // orchestrator mode is not a session
  const n = decodeURIComponent(location.hash.slice(1));
  return /^[A-Za-z0-9_-]{1,64}$/.test(n) ? n : 'main';
}

/** The last list the server gave, for callers that must not wait on the network. */
let lastSessions: SessionView[] = [];

async function sessionList(): Promise<SessionView[]> {
  const r = await fetch('/api/sessions').catch(() => null);
  if (r?.ok) lastSessions = (await r.json()) as SessionView[];
  return r?.ok ? lastSessions : [];
}

// ---------------------------------------------------------------------- panes

const paneEvents: PaneEvents = {
  onUnauthorized: () => showLogin(),
  onActivate: (p) => focus(p),
  onChange: (p, what) => {
    if (what === 'name') {
      sidebar?.reload();
      saveTiles();
    }
    if (what === 'progress' || what === 'attention') sidebar?.reload();
    if (p !== focused) return;
    switch (what) {
      case 'name':
        updateHash();
        rightbar?.setSession(p.name, p.title);
        rightbar?.setStatus(p.statusText);
        break;
      case 'status':
        rightbar?.setStatus(p.statusText);
        break;
      case 'info':
        rightbar?.setInfo(p.info, p.actions);
        break;
      case 'data':
        rightbar?.setData(p.data);
        break;
      case 'agent':
        setAgentAccent(p.agent);
        break;
    }
  },
  onSplit: () => void split(),
  onLayout: () => openLayout(),
  onLaunch: (p, anchor) => void launchMenu(p, anchor),
  onOpenFile: (session, path, line) => {
    layout.dismissOverlays();
    void new FileViewer($('app'), session, path, line, showLogin).open();
  },
  onClose: (p) => closePane(p),
  onMenu: () => toggleList(),
  onPanel: () => layout.toggle('right'),
  onOpenAgent: (session, a) => openAgent(session, a),
  onCommand: () => openCommandBar(),
};

function createPane(): Pane {
  const p = new Pane(paneEvents);
  tilesEl.append(p.el);
  panes.push(p);
  return p;
}

/** Make `p` the pane the sidebars describe and the keyboard types into. */
function focus(p: Pane) {
  if (focused === p) return;
  focused?.setFocused(false);
  focused = p;
  lastFocus.set(p, ++focusTick);
  p.setFocused(true);
  setAgentAccent(p.agent);
  rightbar?.setSession(p.name, p.title);
  rightbar?.setStatus(p.statusText);
  rightbar?.setInfo(p.info, p.actions);
  rightbar?.setData(p.data);
  updateHash();
  sidebar?.reload();
  applyTiles(); // a pane behind a tab takes a tile when it is focused
  saveTiles();
}

function updateHash() {
  if (orchOpen() || isOrchHash()) return; // orchestrator mode owns the address while it is open
  if (focused && location.hash.slice(1) !== focused.name) history.replaceState(null, '', `#${encodeURIComponent(focused.name)}`);
  // An installed app is opened from its icon with no address: remember where you were, so it goes back there.
  if (focused && !focused.isAgent) {
    try {
      localStorage.setItem(LAST_KEY, focused.name);
    } catch {
      /* not remembered: the app opens on main */
    }
  }
}

function lastSession(): string | null {
  try {
    return localStorage.getItem(LAST_KEY);
  } catch {
    return null;
  }
}

function openAgentSheet(session: string, a: SubAgent) {
  new AgentSheet($('app'), session, a, { onUnauthorized: showLogin, open: (c) => openAgent(session, c) }).open();
}

/**
 * Show a helper agent's transcript: in a pane beside the conversation when there is room, or in a
 * sheet over it. From inside a helper's own pane, in place.
 */
function openAgent(session: string, a: SubAgent) {
  if (!canSplit || !focused) return openAgentSheet(session, a);
  const others = panes.filter((p) => p !== focused);
  const target = focused.isAgent
    ? focused
    : (others.find((p) => p.isAgent) ?? others.find((p) => slotOf.includes(p)) ?? (panes.length < limit() ? createPane() : null));
  if (!target) return openAgentSheet(session, a);
  target.openAgent(session, a); // the conversation you were reading keeps focus
  applyTiles();
}

/** Open a session in the focused pane, or jump to the pane that already shows it. */
function openSession(name: string) {
  const open = panes.find((p) => p.name === name && !p.isAgent);
  if (open) return focus(open);
  if (!focused) focus(createPane());
  focused?.open(name);
}

/** Put a session in a new pane. */
function addPane(name: string) {
  const p = createPane();
  p.open(name);
  focus(p);
}

/** Open another session in a new pane: the one you asked for, or the next running one. */
async function split(name?: string) {
  if (!focused || panes.length >= limit() || !canSplit) return;
  let target = name;
  if (!target) {
    const free = (await sessionList()).filter((s) => !panes.some((p) => p.name === s.id));
    target = (free.find((s) => s.status === 'running') ?? free[0])?.id;
  }
  if (!target) return focused?.say('No other session to show. Start one from the sidebar.');
  addPane(target);
}

/** Open a session beside the one you are in: in a new pane, or in place of the next one if full. */
function openInSplit(id: string) {
  if (!canSplit || !focused) return openSession(id);
  const open = panes.find((p) => p.name === id && !p.isAgent);
  if (open) return focus(open);
  if (panes.length < limit()) return void split(id);
  const next = panes[(panes.indexOf(focused) + 1) % panes.length]!;
  next.open(id);
  focus(next);
}

function closePane(p: Pane) {
  if (panes.length < 2) return;
  const at = panes.indexOf(p);
  panes.splice(at, 1);
  lastFocus.delete(p);
  p.dispose();
  if (focused === p) {
    focused = null;
    focus(panes[Math.min(at, panes.length - 1)]!);
  }
  applyTiles();
  saveTiles();
}

// ---------------------------------------------------------------------- tiles

/** How much room the tiles have, counting the wide list as the icon rail it becomes once tiled. */
function room(): { w: number; h: number } {
  const leftNow = layout.placement('left') === 'pinned' ? $('left').offsetWidth : 0;
  const extra = railOpen ? 0 : Math.max(0, leftNow - RAIL_W);
  return { w: $('panes').clientWidth + extra, h: $('panes').clientHeight + (tabsEl.hidden ? 0 : tabsEl.offsetHeight) };
}

/**
 * Lay the panes out: choose the grid that fits, give each tile its pane, show a prompt in empty
 * tiles, tab away the panes that do not fit, and hand out the GPU renderers.
 */
function applyTiles() {
  const max = limit();
  const { w, h } = room();
  const count = panes.length;
  if (!explicit) {
    const start = tileCount(preset(defaultPreset())) >= count ? defaultPreset() : asked;
    asked = count <= 1 ? 'single' : presetForCount(start, count, w, h, max);
  } else if (count > lastCount && lastCount > 0) {
    // A pane was added to a grid picked by hand: grow it if it is full. Otherwise a grid chosen
    // with more panes open than tiles stays as it is, and the rest wait behind tabs.
    asked = presetForCount(asked, count, w, h, max);
  }
  lastCount = count;
  current = effectivePreset(asked, w, h, max);
  canSplit = max >= 2 && usable(w, h, max).length > 1;
  if (!canSplit) railOpen = false;

  const names = tileNames(current);
  const visible = visiblePanes(count, Math.max(0, focused ? panes.indexOf(focused) : 0), names.length);
  slotOf = names.map((_, s) => panes[visible[s]!] ?? null);
  for (const p of panes) {
    const slot = slotOf.indexOf(p);
    p.el.hidden = slot < 0;
    if (slot >= 0) p.el.style.gridArea = names[slot]!;
  }

  const sz = sizes.get(current.id) ?? sanitizeSizes(current, null, null);
  sizes.set(current.id, sz);
  tilesEl.style.gridTemplateAreas = current.areas.map((r) => `"${r.join(' ')}"`).join(' ');
  applyGrid(sz);
  renderEmpty(names.slice(visible.length));
  renderDividers();
  renderTabs();

  // The list and details buttons live in the outer headers: top-left tile and top-right tile.
  const shown = slotOf.filter((p): p is Pane => p !== null);
  const corner = corners(current);
  const first = slotOf[names.indexOf(corner.first)] ?? shown[0];
  const last = slotOf[names.indexOf(corner.last)] ?? shown[shown.length - 1];
  for (const p of panes) {
    p.el.classList.toggle('is-first', p === first);
    p.el.classList.toggle('is-last', p === last);
  }
  const app = $('app');
  app.classList.toggle('is-split', names.length >= 2);
  app.classList.toggle('can-split', canSplit);
  app.classList.toggle('can-add', canSplit && count < max);
  app.classList.toggle('many', count >= 2);
  app.classList.toggle('rail-open', railOpen);
  balanceGpu();
}

function applyGrid(sz: Sizes) {
  // Scaled so the weights add up to far more than 1: a grid track set in fr only fills the space when they do.
  const track = (n: number) => `minmax(0, ${(n * 1000).toFixed(1)}fr)`;
  tilesEl.style.gridTemplateColumns = sz.cols.map(track).join(' ');
  tilesEl.style.gridTemplateRows = sz.rows.map(track).join(' ');
}

let dividerEls: { d: Divider; el: HTMLElement }[] = [];

function renderDividers() {
  for (const { el: e } of dividerEls) e.remove();
  dividerEls = dividers(current).map((d) => {
    const handle = el('div', `tile-div ${d.axis}`);
    handle.setAttribute('role', 'separator');
    handle.addEventListener('pointerdown', (e) => dragDivider(e, d, handle));
    tilesEl.append(handle);
    return { d, el: handle };
  });
  placeDividers();
}

function placeDividers() {
  const sz = sizes.get(current.id);
  if (!sz) return;
  for (const { d, el: e } of dividerEls) {
    const along = d.axis === 'col' ? sz.cols : sz.rows;
    const across = d.axis === 'col' ? sz.rows : sz.cols;
    const at = span(along, 0, d.after)[1] * 100;
    const [from, to] = span(across, d.from, d.to);
    if (d.axis === 'col') {
      e.style.cssText = `left:${at}%;top:${from * 100}%;height:${(to - from) * 100}%`;
    } else {
      e.style.cssText = `top:${at}%;left:${from * 100}%;width:${(to - from) * 100}%`;
    }
  }
}

function dragDivider(e: PointerEvent, d: Divider, handle: HTMLElement) {
  e.preventDefault();
  const sz = sizes.get(current.id);
  if (!sz) return;
  const id = current.id;
  const start = d.axis === 'col' ? e.clientX : e.clientY;
  const total = d.axis === 'col' ? tilesEl.clientWidth : tilesEl.clientHeight;
  const before = (d.axis === 'col' ? sz.cols : sz.rows).slice();
  handle.setPointerCapture(e.pointerId);
  handle.classList.add('dragging');
  const move = (ev: PointerEvent) => {
    const now = d.axis === 'col' ? ev.clientX : ev.clientY;
    const next = dragSizes(before, d.after, (now - start) / total);
    const sizesNow: Sizes = d.axis === 'col' ? { cols: next, rows: sz.rows } : { cols: sz.cols, rows: next };
    sizes.set(id, sizesNow);
    applyGrid(sizesNow);
    placeDividers();
  };
  const stop = () => {
    handle.classList.remove('dragging');
    handle.removeEventListener('pointermove', move);
    handle.removeEventListener('pointerup', stop);
    handle.removeEventListener('pointercancel', stop);
    explicit = true;
    saveTiles();
  };
  handle.addEventListener('pointermove', move);
  handle.addEventListener('pointerup', stop);
  handle.addEventListener('pointercancel', stop);
}

/** Tiles with no pane yet offer the sessions that are not on screen. */
function renderEmpty(empty: string[]) {
  for (const n of tilesEl.querySelectorAll('.tile-empty')) n.remove();
  if (!empty.length) return;
  const boxes = empty.map((area) => {
    const box = el('div', 'tile-empty');
    box.style.gridArea = area;
    box.append(el('div', 'te-title', 'Empty tile'));
    tilesEl.append(box);
    return box;
  });
  void sessionList().then((list) => {
    const free = list.filter((s) => !panes.some((p) => p.name === s.id)).slice(0, 8);
    for (const box of boxes) {
      if (!box.isConnected) continue;
      const row = el('div', 'te-chips');
      for (const s of free) {
        row.append(chip({ onClick: () => addPane(s.id) }, s.id));
      }
      box.append(row.childElementCount ? row : el('div', 'te-hint', 'No other sessions. Start one from the list.'));
    }
  });
}

/** Panes that do not fit sit behind a strip of tabs; tapping one brings it on screen. */
function renderTabs() {
  const overflow = panes.length > slotOf.length;
  tabsEl.hidden = !overflow;
  if (!overflow) return tabsEl.replaceChildren();
  tabsEl.replaceChildren(
    ...panes.map((p) => {
      const b = el('button', 'tile-tab', p.title || p.name);
      b.type = 'button';
      b.classList.toggle('shown', slotOf.includes(p));
      b.setAttribute('aria-pressed', String(p === focused));
      b.addEventListener('click', () => focus(p));
      return b;
    }),
  );
}

/** The focused pane and the most recently used ones draw on the GPU; the rest use the DOM renderer. */
function balanceGpu() {
  const ranked = panes.filter((p) => !p.el.hidden).sort((a, b) => (lastFocus.get(b) ?? 0) - (lastFocus.get(a) ?? 0));
  const gpu = new Set(ranked.slice(0, GPU_BUDGET));
  for (const p of panes) p.setGpu(gpu.has(p));
}

let tilesQueued = false;
function queueTiles() {
  if (tilesQueued) return;
  tilesQueued = true;
  requestAnimationFrame(() => {
    tilesQueued = false;
    if (!$('app').hidden) applyTiles();
  });
}
new ResizeObserver(queueTiles).observe($('panes'));

/** The list button: in tiled view the rail widens; otherwise the sidebar slides in or out. */
function toggleList() {
  if ($('app').classList.contains('is-split') && layout.placement('left') === 'pinned') {
    railOpen = !railOpen;
    applyTiles();
  } else {
    layout.toggle('left');
  }
}

// ---------------------------------------------------------------------- command bar

const RECENT_COMMANDS_KEY = 'ttm.cmd.recent';

function recentCommands(): string[] {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(RECENT_COMMANDS_KEY) ?? '[]');
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

async function getJson<T>(url: string, fallback: T): Promise<T> {
  const r = await fetch(url).catch(() => null);
  if (r?.status === 401) showLogin();
  return r?.ok ? ((await r.json()) as T) : fallback;
}

let presetCache: Promise<PresetInfo[]> | null = null;
const presets = () => (presetCache ??= fetchPresets());

/** The folder a pane's session belongs to: where it was started, else where it is. */
async function projectOf(pane: Pane | null): Promise<string | null> {
  if (!pane) return null;
  const mine = (await sessionList().catch(() => [])).find((s) => s.id === pane.name);
  return mine?.project ?? mine?.cwd ?? null;
}

/** Same, from the last list seen, with no wait. */
function knownProjectOf(pane: Pane | null): string | null {
  const mine = pane && lastSessions.find((s) => s.id === pane.name);
  return mine ? mine.project ?? mine.cwd ?? null : null;
}

/** Start a session in `dir` and show it in the focused pane. */
async function startHere(preset: string | undefined, dir: string) {
  const r = await launchSession({ preset, yolo: false, dir });
  if (!r.ok) {
    if (r.signedOut) return showLogin();
    return focused?.say(r.error);
  }
  sidebar?.reload();
  layout.dismissOverlays();
  openSession(r.id);
}

/** The new-session sheet: the focused pane's project is filled in unless the caller names a folder. */
async function newSession(dir?: string, agent?: 'claude' | 'codex') {
  if (document.querySelector(SHEET_SELECTOR)) return;
  layout.dismissOverlays();
  // The sheet opens on the tap: the pane's folder comes from the list already on hand, and only a cold start waits.
  const from = dir ?? knownProjectOf(focused) ?? (lastSessions.length === 0 ? await projectOf(focused) : null) ?? undefined;
  void new NewSessionSheet($('app'), showLogin, {
    opened: (id) => {
      sidebar?.reload();
      layout.dismissOverlays();
      openSession(id);
    },
    pinned: () => layouts.pinnedProjects,
  }).open(from, agent);
}

/** The pane header's and the key bar's New button: Claude, Codex or a shell in this pane's project. */
async function launchMenu(pane: Pane, anchor: HTMLElement) {
  const dir = await projectOf(pane);
  if (!dir) return void newSession();
  const { claude, codex, more } = mainAgents(await presets());
  const name = dir.split('/').filter(Boolean).pop() ?? dir;
  const go = (p?: PresetInfo) => () => void startHere(p?.name, dir);
  const items: MenuItem[] = [];
  if (claude) items.push({ label: 'Claude', hint: `in ${name}`, run: go(claude) });
  if (codex) items.push({ label: 'Codex', hint: `in ${name}`, run: go(codex) });
  items.push({ label: 'Shell', hint: `in ${name}`, run: go() });
  for (const p of more) items.push({ label: p.name, hint: `in ${name}`, run: go(p) });
  items.push({ label: 'More options…', hint: 'skip permissions, other folder', run: () => void newSession(dir) });
  openMenu(anchor, items, `New session in ${name}`);
}

/** Keep a project in the sidebar when nothing runs in it, or stop. Stored with the layouts, so every device sees it. */
async function togglePinProject(path: string) {
  const next = await updateLayouts((l) => ({
    ...l,
    pinnedProjects: l.pinnedProjects.includes(path) ? l.pinnedProjects.filter((p) => p !== path) : [...l.pinnedProjects, path],
  }));
  if (next) layouts = next;
}

const commandCtx: Ctx = {
  focused: () => focused,
  panes: () => panes.filter((p) => !p.isAgent),
  canSplit: () => canSplit,
  paneLimit: limit,
  sessions: sessionList,
  presets: presets,
  focusPane: focus,
  switchTo: openSession,
  startHere: (preset, dir) => startHere(preset, dir),
  newSession: (dir) => void newSession(dir),
  showAnother: () => void split(),
  closePane,
  openLayout,
  openSettings,
  openFile: (session, path) => paneEvents.onOpenFile(session, path, null),
};

const commandBar = new CommandBar($('app'), {
  recent: recentCommands,
  used: (id) => {
    try {
      localStorage.setItem(RECENT_COMMANDS_KEY, JSON.stringify(remember(recentCommands(), id)));
    } catch {
      /* the order just is not remembered */
    }
  },
  closed: () => focused?.focusTerm(),
});

function openCommandBar() {
  if ($('app').hidden || document.querySelector(SHEET_SELECTOR)) return;
  layout.dismissOverlays();
  commandBar.open(rootStep(commandCtx));
}

// Cmd+K (Mac, iPad) or Ctrl+K opens it, even while the terminal has the keyboard. Pressing Ctrl+K
// again closes it and sends the terminal its Ctrl+K (kill to end of line), which the bar took.
document.addEventListener(
  'keydown',
  (e) => {
    if (e.code !== 'KeyK' || e.altKey || e.shiftKey || e.metaKey === e.ctrlKey || $('app').hidden) return;
    if (!commandBar.isOpen && document.querySelector(SHEET_SELECTOR)) return;
    e.preventDefault();
    e.stopPropagation();
    if (!commandBar.isOpen) return openCommandBar();
    commandBar.close();
    if (e.ctrlKey) focused?.sendRaw('\x0b');
  },
  true,
);

// Ctrl+Alt+arrows move between tiles; Ctrl+Alt+1..6 jump to one; Ctrl+Alt+F searches the history; Ctrl+Alt+[ and ] step through your messages.
document.addEventListener(
  'keydown',
  (e) => {
    if (!e.ctrlKey || !e.altKey || e.metaKey || e.shiftKey || $('app').hidden || document.querySelector(SHEET_SELECTOR)) return;
    if (e.code === 'KeyF') {
      e.preventDefault();
      e.stopPropagation();
      focused?.openSearch();
      return;
    }
    if (e.code === 'BracketLeft' || e.code === 'BracketRight') {
      e.preventDefault();
      e.stopPropagation();
      focused?.stepTurns(e.code === 'BracketLeft' ? 'older' : 'newer');
      return;
    }
    const dirs: Record<string, Dir> = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down' };
    let slot: number | null = null;
    const dir = dirs[e.key];
    if (dir) slot = neighbor(current, Math.max(0, focused ? slotOf.indexOf(focused) : 0), dir);
    else if (/^Digit[1-6]$/.test(e.code)) slot = Number(e.code.slice(5)) - 1;
    const target = slot === null ? null : slotOf[slot];
    if (!target) return;
    e.preventDefault();
    e.stopPropagation(); // the terminal must not see it
    focus(target);
  },
  true,
);

// ---------------------------------------------------------------------- remembering

interface SavedTiles {
  names: string[];
  focus: number;
  asked: PresetId;
  explicit: boolean;
}

function saveTiles() {
  // A helper's transcript is a passing view, not part of the layout to come back to.
  if (panes.some((p) => p.isAgent)) return;
  try {
    const memo: Record<string, Sizes> = {};
    for (const [id, s] of sizes) memo[id] = s;
    localStorage.setItem(
      TILES_KEY,
      JSON.stringify({ names: panes.map((p) => p.name), focus: Math.max(0, panes.indexOf(focused!)), asked, explicit, sizes: memo }),
    );
    localStorage.removeItem(OLD_SPLIT_KEY);
  } catch {
    /* not remembered */
  }
}

function loadTiles(): SavedTiles | null {
  try {
    const raw = JSON.parse(localStorage.getItem(TILES_KEY) ?? 'null') as Record<string, unknown> | null;
    const valid = (n: unknown): n is string => typeof n === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(n);
    if (raw && Array.isArray(raw.names)) {
      const names = raw.names.filter(valid).slice(0, MAX_PANES_LIMIT);
      if (!names.length) return null;
      if (raw.sizes && typeof raw.sizes === 'object') {
        for (const [id, s] of Object.entries(raw.sizes as Record<string, { cols?: unknown; rows?: unknown }>)) {
          if (isPresetId(id)) sizes.set(id, sanitizeSizes(preset(id), s?.cols, s?.rows));
        }
      }
      return {
        names,
        focus: typeof raw.focus === 'number' ? raw.focus : 0,
        asked: isPresetId(raw.asked) ? raw.asked : 'cols2',
        explicit: raw.explicit === true,
      };
    }
    // Before tiling, only a two-pane split was remembered.
    const old = JSON.parse(localStorage.getItem(OLD_SPLIT_KEY) ?? 'null') as { names?: unknown; focus?: unknown } | null;
    const names = Array.isArray(old?.names) ? old.names.filter(valid) : [];
    if (names.length === 2) return { names, focus: old?.focus === 1 ? 1 : 0, asked: 'cols2', explicit: false };
  } catch {
    /* ignore */
  }
  return null;
}

// ---------------------------------------------------------------------- layout sheet and workspaces

let layoutSheet: LayoutSheet | null = null;

function openLayout() {
  layout.dismissOverlays();
  layoutSheet ??= new LayoutSheet($('app'), {
    onUnauthorized: showLogin,
    active: () => current.id,
    usable: () => {
      const { w, h } = room();
      return usable(w, h, limit()).map((p) => p.id);
    },
    choose: (id) => {
      asked = id;
      explicit = true;
      applyTiles();
      saveTiles();
    },
    snapshot,
    open: (w) => void openWorkspace(w),
    workspaces: () => layouts.workspaces,
    saved: (l) => {
      layouts = l;
    },
  });
  layoutSheet.open();
}

/** The grid and the session in each tile, as a workspace called `name`. */
function snapshot(name: string): Workspace {
  const sz = sizes.get(current.id) ?? sanitizeSizes(current, null, null);
  return { name, preset: current.id, cols: sz.cols, rows: sz.rows, tiles: slotOf.map((p) => (p && !p.isAgent ? p.name : '')) };
}

/** Bring a workspace back: its grid, its sizes and its sessions. Sessions that are gone are skipped. */
async function openWorkspace(w: Workspace) {
  const alive = new Set((await sessionList()).map((s) => s.id));
  const wanted = [...new Set(w.tiles.filter(Boolean))];
  const names = wanted.filter((n) => alive.has(n)).slice(0, limit());
  const missing = wanted.filter((n) => !alive.has(n));
  if (!names.length) return focused?.say('None of that workspace’s sessions are running.');
  asked = w.preset;
  explicit = true;
  sizes.set(w.preset, sanitizeSizes(preset(w.preset), w.cols, w.rows));
  while (panes.length > names.length) {
    const p = panes.pop()!;
    lastFocus.delete(p);
    p.dispose();
  }
  while (panes.length < names.length) createPane();
  names.forEach((n, i) => {
    const p = panes[i]!;
    if (p.name !== n || p.isAgent) p.open(n);
  });
  focused?.setFocused(false);
  focused = null;
  focus(panes[0]!);
  applyTiles();
  saveTiles();
  if (missing.length) panes[0]?.say(`Not running, skipped: ${missing.join(', ')}`);
}

// ---------------------------------------------------------------------- app

function showLogin() {
  // Hide the app first: everything below (and the layout's own listeners) checks it, and each of
  // many requests failing at once with a 401 calls this again.
  const wasShowing = !$('app').hidden;
  $('app').hidden = true;
  $('offline').hidden = true;
  $('login').hidden = false;
  if (wasShowing) {
    for (const p of panes) p.dispose();
    panes.length = 0;
    slotOf = [];
    lastCount = 0;
    lastFocus.clear();
    focused = null;
    setAgentAccent(null);
    $('app').classList.remove('is-split', 'many', 'can-add');
    usage?.stop();
    sidebar?.setActive(false);
    rightbar?.setActive(false);
    layout.dismissOverlays();
  }
  $<HTMLInputElement>('pw').focus();
}

async function logout() {
  await fetch('/api/logout', { method: 'POST' }).catch(() => null);
  showLogin();
}

/**
 * Act on a launch: the page's own address when it was opened, or the address an already-open installed app is
 * handed when you tap a shortcut or a notification (the launch queue; the manifest asks for focus-existing).
 * `fromQueue` launches also switch to the session they name; a cold start has done that already.
 */
let lastLaunch: { launch: Launch; at: number } | null = null;
function handleLaunch(launch: Launch, fromQueue: boolean) {
  // The page's address and the launch queue both report a cold start: act once.
  if (lastLaunch && Date.now() - lastLaunch.at < 3000 && sameLaunch(lastLaunch.launch, launch)) return;
  lastLaunch = { launch, at: Date.now() };
  if (location.search) history.replaceState(null, '', location.pathname + location.hash); // ?new= must not stick
  if (fromQueue && launch.session && focused?.name !== launch.session) openSession(launch.session);
  if (launch.newAgent) void newSession(undefined, launch.newAgent);
  if (launch.resume) void resumeWhenReady();
}

/** The conversation list needs the focused session's chat to have loaded; a launch can arrive before that. */
async function resumeWhenReady() {
  for (let i = 0; i < 20; i++) {
    if (focused?.agent && focused.actions) return focused.actions.resume();
    await new Promise((r) => setTimeout(r, 250));
  }
  focused?.say('Open a Claude or Codex session to resume a conversation.');
}

let launchQueueSet = false;
function watchLaunches() {
  handleLaunch(parseLaunch(location.href), false);
  const queue = (window as { launchQueue?: { setConsumer(cb: (p: { targetURL?: string }) => void): void } }).launchQueue;
  if (!queue || launchQueueSet) return;
  launchQueueSet = true;
  queue.setConsumer((p) => {
    if (p.targetURL && !$('app').hidden) handleLaunch(parseLaunch(p.targetURL), true);
  });
}

async function showApp() {
  $('login').hidden = true;
  $('offline').hidden = true;
  $('app').hidden = false;
  syncOrchRoute(); // the page may have been opened straight onto #orch/<run>
  void loadNewSessionData();
  watchForUpdates($('app'));
  watchLaunches();
  usage ??= new UsageView($('usage'), showLogin);
  sidebar ??= new SessionSidebar(
    $('left'),
    () => focused?.name ?? '',
    openSession,
    showLogin,
    {
      navigated: () => {
        layout.dismissOverlays();
        // Picking a session from the widened list puts the rail back, so the panes get their room again.
        if (railOpen) {
          railOpen = false;
          applyTiles();
        }
      },
      collapse: () => toggleList(),
      openSettings,
      newSession: (dir) => void newSession(dir),
      pinnedProjects: () => layouts.pinnedProjects,
      togglePinProject,
      openInSplit,
      openAgent,
      shown: () => panes.map((p) => p.name),
      progress: (id) => panes.find((p) => p.name === id)?.el.dataset.progress || null,
      attention: (id) => !!panes.find((p) => p.name === id)?.el.dataset.attention,
    },
    (id) => usage?.tokensLabel(id),
  );
  rightbar ??= new RightBar($('right'), usage.body, {
    collapse: () => layout.toggle('right'),
    refreshUsage: () => usage?.refreshNow(),
    search: (q) => focused?.search(q) ?? [],
    openAgent: (a) => {
      if (focused) openAgent(focused.name, a);
    },
    jump: (id) => {
      layout.dismissOverlays(); // on a phone the sidebar would cover the message
      focused?.showMessage(id);
    },
    sheetHost: () => $('app'),
    signedOut: () => showLogin(),
  });
  usage.start(); // before syncPanels: start() resets what the sidebar asked the usage view to watch
  syncPanels();
  layouts = await fetchLayouts();
  // The address names a session; failing that (the icon), the last one you had open, if it is still there.
  const list = await sessionList();
  const known = (list.length > 0 ? list : readSessionCache())?.map((s) => s.id) ?? null; // offline: what we knew
  const name = startSession(parseLaunch(location.href).session, lastSession(), known);
  const saved = loadTiles();
  if (saved && saved.names.length >= 2 && limit() >= 2 && saved.names.includes(name)) {
    // Bring back the panes you had, focused on the one the address names.
    asked = saved.asked;
    explicit = saved.explicit;
    const made = saved.names.slice(0, limit()).map((n) => {
      const p = createPane();
      p.open(n);
      return p;
    });
    focus(made.find((p) => p.name === name) ?? made[saved.focus] ?? made[0]!);
  } else {
    openSession(name);
  }
  applyTiles();
}

function openSettings() {
  layout.dismissOverlays(); // a sheet would sit under an open sidebar
  settings ??= new SettingsSheet($('app'), {
    onUnauthorized: showLogin,
    logout: () => void logout(),
    currentSession: () => focused?.name ?? '',
    terminalFont: () => focused?.terminalFont ?? null,
    setTerminalFont: (px) => {
      for (const p of panes) p.setTerminalFont(px);
    },
    layoutSettings: () => layouts.settings,
    setLayoutSettings: (next) => {
      layouts = { ...layouts, settings: next };
      applyTiles();
      void updateLayouts((l) => ({ ...l, settings: next }));
    },
  });
  settings.open();
}

/** Poll and refresh only what is on screen, keep hidden sidebars out of the tab order, and decide
 *  whether there is room for two panes. */
function syncPanels() {
  const left = layout.isVisible('left');
  const right = layout.isVisible('right');
  const on = !$('app').hidden; // signed out: nothing polls
  applyTiles();
  sidebar?.setActive(left && on);
  usage?.watch(right && on);
  rightbar?.setActive(right && on);
  $('left').inert = !left;
  $('right').inert = !right;
}
layout.onChange(syncPanels);

// A sidebar that has slid over the page is an overlay too: Back closes it.
let releaseSidebar: (() => void) | null = null;
layout.onChange(() => {
  const app = $('app');
  const over = app.classList.contains('l-open') || app.classList.contains('r-open');
  if (over && !releaseSidebar) releaseSidebar = pushOverlay(() => layout.dismissOverlays());
  else if (!over && releaseSidebar) {
    releaseSidebar();
    releaseSidebar = null;
  }
});

$<HTMLFormElement>('login-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const pw = $<HTMLInputElement>('pw');
  const r = await fetch('/api/login', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ password: pw.value }),
  }).catch(() => null);
  pw.value = '';
  if (r?.ok) {
    $('login-err').textContent = '';
    showApp();
  } else {
    $('login-err').textContent =
      r?.status === 429 ? 'Too many attempts, wait a minute' : r ? 'Wrong password' : 'Server unreachable';
  }
});

const nudgeAll = () => {
  for (const p of panes) p.nudge();
  usage?.nudge();
};
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) nudgeAll();
});
window.addEventListener('online', nudgeAll);
window.addEventListener('hashchange', () => {
  if (!$('app').hidden) syncOrchRoute();
  if (isOrchHash()) return; // `#orch` is a mode, not a session
  if (isOverlayTraversal()) return; // an overlay's history entry going away, not a request to open another session
  if (focused && sessionName() !== focused.name && !$('app').hidden) openSession(sessionName());
});
// After an overlay's entry goes away the address can name the session from before it opened: make it name this one.
onAddressRestore(updateHash);
$('usage').addEventListener('pointerdown', (e) => e.preventDefault());
$('usage').addEventListener('click', () => layout.set('right', true));
$('scrim').addEventListener('click', () => layout.dismissOverlays());
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !document.querySelector(SHEET_SELECTOR)) layout.dismissOverlays();
});
edgeSwipe({
  app: $('app'),
  layout,
  panels: { left: $('left'), right: $('right') },
  scrim: $('scrim'),
  blocked: () => !!$('app').hidden || document.querySelector(`${SHEET_SELECTOR}, .img-viewer`) !== null,
});

// iOS Safari ignores interactive-widget: size the app to the *visual* viewport so the
// keybar sits above the soft keyboard.
const vv = window.visualViewport;
if (vv) {
  // The keyboard animation fires resize and scroll many times a frame; relayout once per frame, and only when the
  // height really changed.
  let last = '';
  let queued = false;
  const apply = () => {
    queued = false;
    // Follow the visual viewport only while something (the keyboard, a pinch) has really shrunk it. With none, the
    // layout height is right, and an installed iOS app can report a visual height a few dozen points short.
    const next = window.innerHeight - vv.height > 120 ? `${vv.height}px` : '';
    if (next !== last) {
      last = next;
      if (next) document.documentElement.style.setProperty('--vvh', next);
      else document.documentElement.style.removeProperty('--vvh');
    }
    if (window.scrollY !== 0 || window.scrollX !== 0) window.scrollTo(0, 0);
  };
  const sync = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(apply);
  };
  vv.addEventListener('resize', sync);
  vv.addEventListener('scroll', sync);
  apply();
}

if ('serviceWorker' in navigator) {
  // A tapped notification asks the already-open page to switch sessions.
  navigator.serviceWorker.addEventListener('message', (e: MessageEvent) => {
    const d = e.data as { type?: string; session?: string } | null;
    if (d?.type === 'open' && d.session && !$('app').hidden) openSession(d.session);
    // A usage-limit warning was tapped: show the numbers instead of a session.
    if (d?.type === 'usage' && !$('app').hidden) layout.set('right', true);
  });
  window.addEventListener('load', () => navigator.serviceWorker.register('/sw.js').catch(() => {}));
}

// Start up. Only a real "not signed in" shows the password box: a server that cannot be reached (the tunnel is
// down, the phone has no signal) gets a screen that says so and keeps trying.
let offlineTimer = 0;
async function boot() {
  window.clearTimeout(offlineTimer);
  const r = await fetch('/api/me').catch(() => null);
  if (r?.ok) return void showApp();
  if (r?.status === 401) return showLogin();
  // Signed in before (there is a list from last time): open the app on it. Its panes reconnect and the sidebar
  // refreshes on their own once the server answers; a session that has expired shows the login then.
  if (readSessionCache()) return void showApp();
  $('login').hidden = true;
  $('offline').hidden = false;
  offlineTimer = window.setTimeout(() => void boot(), 5000);
}
const retryBoot = () => {
  if (!$('offline').hidden) void boot();
};
$('offline-retry').addEventListener('click', retryBoot);
window.addEventListener('online', retryBoot);
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) retryBoot();
});
void boot();
