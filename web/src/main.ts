import '@fontsource-variable/inter/wght.css';
import '@fontsource-variable/jetbrains-mono/wght.css';
import './style.css';
import { initTheme, setAgentAccent } from './theme';
import { initUiScale } from './scale';
import { SessionSidebar, type SessionView } from './sessions';
import { UsageView } from './usage';
import { Layout } from './layout';
import { edgeSwipe } from './gestures';
import { RightBar } from './rightbar';
import { SettingsSheet } from './settings';
import { Pane, type PaneEvents } from './pane';
import { AgentSheet } from './agent-sheet';
import type { SubAgent } from './agents';

initTheme();
initUiScale();

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const layout = new Layout($('app'));
let sidebar: SessionSidebar | null = null;
let rightbar: RightBar | null = null;
let settings: SettingsSheet | null = null;
let usage: UsageView | null = null;

/** The open panes: one, or two side by side on a wide screen. */
const panes: Pane[] = [];
let focused: Pane | null = null;
/** The screen is wide enough for two panes. */
let canSplit = false;
/** In split view the session list is an icon rail; this widens it back to the full list. */
let railOpen = false;

const SPLIT_KEY = 'ttm.split';

function sessionName(): string {
  const n = decodeURIComponent(location.hash.slice(1));
  return /^[A-Za-z0-9_-]{1,64}$/.test(n) ? n : 'main';
}

// ---------------------------------------------------------------------- panes

const paneEvents: PaneEvents = {
  onUnauthorized: () => showLogin(),
  onActivate: (p) => focus(p),
  onChange: (p, what) => {
    if (what === 'name') {
      sidebar?.reload();
      saveSplit();
    }
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
  onClose: (p) => closePane(p),
  onMenu: () => toggleList(),
  onPanel: () => layout.toggle('right'),
  onOpenAgent: (session, a) => openAgent(session, a),
};

function createPane(): Pane {
  const p = new Pane(paneEvents);
  $('panes').append(p.el);
  panes.push(p);
  applySplit();
  return p;
}

/** Make `p` the pane the sidebars describe and the keyboard types into. */
function focus(p: Pane) {
  if (focused === p) return;
  focused?.setFocused(false);
  focused = p;
  p.setFocused(true);
  setAgentAccent(p.agent);
  rightbar?.setSession(p.name, p.title);
  rightbar?.setStatus(p.statusText);
  rightbar?.setInfo(p.info, p.actions);
  rightbar?.setData(p.data);
  updateHash();
  sidebar?.reload();
  saveSplit();
}

function updateHash() {
  if (focused && location.hash.slice(1) !== focused.name) history.replaceState(null, '', `#${encodeURIComponent(focused.name)}`);
}

/**
 * Show a helper agent's transcript: beside the conversation when there is room (replacing the other
 * pane if two are open), or in a sheet over it. From inside a helper's own pane, in place.
 */
function openAgent(session: string, a: SubAgent) {
  if (!canSplit || !focused) {
    new AgentSheet($('app'), session, a, { onUnauthorized: showLogin, open: (c) => openAgent(session, c) }).open();
    return;
  }
  const target = focused.isAgent ? focused : (panes.find((p) => p !== focused) ?? createPane());
  target.openAgent(session, a); // the conversation you were reading keeps focus
  applySplit();
}

/** Open a session in the focused pane. */
function openSession(name: string) {
  if (!focused) focus(createPane());
  focused?.open(name);
}

/** Put the focused pane's neighbour on screen: a second session beside this one. */
async function split(name?: string) {
  if (!focused || panes.length >= 2 || !canSplit) return;
  let target = name;
  if (!target) {
    const r = await fetch('/api/sessions').catch(() => null);
    const list = r?.ok ? ((await r.json()) as SessionView[]) : [];
    const others = list.filter((s) => s.id !== focused?.name);
    target = (others.find((s) => s.status === 'running') ?? others[0])?.id;
  }
  if (!target) return focused.say('No other session to show. Start one from the sidebar.');
  const p = createPane();
  p.open(target);
  focus(p);
  applySplit();
}

/** Open a session beside the one you are in: in the other pane, or a new one. */
function openInSplit(id: string) {
  if (!canSplit || !focused) return openSession(id);
  const other = panes.find((p) => p !== focused);
  if (other) {
    other.open(id);
    focus(other);
  } else {
    void split(id);
  }
}

function closePane(p: Pane) {
  if (panes.length < 2) return;
  const rest = panes.filter((x) => x !== p);
  panes.splice(panes.indexOf(p), 1);
  p.dispose();
  if (focused === p) {
    focused = null;
    focus(rest[0]!);
  }
  applySplit();
  saveSplit();
}

/** The layout classes, and which panes show: with two open but no room, only the focused one does. */
function applySplit() {
  const squeezed = panes.length === 2 && !canSplit;
  for (const p of panes) p.el.hidden = squeezed && p !== focused;
  const shown = panes.filter((p) => !p.el.hidden);
  // The list and details buttons live in the outer headers: first pane on the left, last on the right.
  for (const p of panes) {
    p.el.classList.toggle('is-first', p === shown[0]);
    p.el.classList.toggle('is-last', p === shown[shown.length - 1]);
  }
  $('app').classList.toggle('is-split', shown.length === 2);
  $('app').classList.toggle('can-split', canSplit);
  $('app').classList.toggle('rail-open', railOpen);
}

/** The list button: in split view the rail widens; otherwise the sidebar slides in or out. */
function toggleList() {
  if ($('app').classList.contains('is-split') && layout.placement('left') === 'pinned') {
    railOpen = !railOpen;
    applySplit();
  } else {
    layout.toggle('left');
  }
}

function saveSplit() {
  // On a screen too narrow for two panes the app shows one: don't let that wipe the split you had.
  // A helper's transcript is a passing view, not part of the layout to come back to.
  if (!canSplit || panes.some((p) => p.isAgent)) return;
  try {
    if (panes.length === 2) {
      localStorage.setItem(SPLIT_KEY, JSON.stringify({ names: panes.map((p) => p.name), focus: panes.indexOf(focused!) }));
    } else {
      localStorage.removeItem(SPLIT_KEY);
    }
  } catch {
    /* not remembered */
  }
}

function loadSplit(): { names: string[]; focus: number } | null {
  try {
    const raw = JSON.parse(localStorage.getItem(SPLIT_KEY) ?? 'null') as { names?: unknown; focus?: unknown } | null;
    const names = Array.isArray(raw?.names) ? raw.names.filter((n): n is string => typeof n === 'string') : [];
    if (names.length === 2 && names.every((n) => /^[A-Za-z0-9_-]{1,64}$/.test(n))) return { names, focus: raw?.focus === 1 ? 1 : 0 };
  } catch {
    /* ignore */
  }
  return null;
}

// ---------------------------------------------------------------------- app

function showLogin() {
  // Hide the app first: everything below (and the layout's own listeners) checks it, and each of
  // many requests failing at once with a 401 calls this again.
  const wasShowing = !$('app').hidden;
  $('app').hidden = true;
  $('login').hidden = false;
  if (wasShowing) {
    for (const p of panes) p.dispose();
    panes.length = 0;
    focused = null;
    setAgentAccent(null);
    $('app').classList.remove('is-split');
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

function showApp() {
  $('login').hidden = true;
  $('app').hidden = false;
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
          applySplit();
        }
      },
      collapse: () => toggleList(),
      openSettings,
      openInSplit,
      openAgent,
      shown: () => panes.map((p) => p.name),
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
  });
  usage.start(); // before syncPanels: start() resets what the sidebar asked the usage view to watch
  syncPanels();
  const name = sessionName();
  const saved = loadSplit();
  if (saved && canSplit && saved.names.includes(name)) {
    // Bring back the two panes you had, focused on the one the address names.
    const [a, b] = [createPane(), createPane()];
    a.open(saved.names[0]!);
    b.open(saved.names[1]!);
    focus(saved.names[1] === name ? b : a);
    applySplit();
  } else {
    openSession(name);
  }
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
  });
  settings.open();
}

/** Poll and refresh only what is on screen, keep hidden sidebars out of the tab order, and decide
 *  whether there is room for two panes. */
function syncPanels() {
  const left = layout.isVisible('left');
  const right = layout.isVisible('right');
  const on = !$('app').hidden; // signed out: nothing polls
  canSplit = layout.placement('right') === 'pinned';
  if (!canSplit) railOpen = false;
  applySplit();
  sidebar?.setActive(left && on);
  usage?.watch(right && on);
  rightbar?.setActive(right && on);
  $('left').inert = !left;
  $('right').inert = !right;
}
layout.onChange(syncPanels);

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
  if (focused && sessionName() !== focused.name && !$('app').hidden) openSession(sessionName());
});
$('usage').addEventListener('pointerdown', (e) => e.preventDefault());
$('usage').addEventListener('click', () => layout.set('right', true));
$('scrim').addEventListener('click', () => layout.dismissOverlays());
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !document.querySelector('.sheet')) layout.dismissOverlays();
});
edgeSwipe({
  app: $('app'),
  layout,
  panels: { left: $('left'), right: $('right') },
  scrim: $('scrim'),
  blocked: () => !!$('app').hidden || document.querySelector('.sheet') !== null,
});

// iOS Safari ignores interactive-widget: size the app to the *visual* viewport so the
// keybar sits above the soft keyboard.
const vv = window.visualViewport;
if (vv) {
  const sync = () => {
    document.documentElement.style.setProperty('--vvh', `${vv.height}px`);
    window.scrollTo(0, 0);
  };
  vv.addEventListener('resize', sync);
  vv.addEventListener('scroll', sync);
  sync();
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

fetch('/api/me').then((r) => (r.ok ? showApp() : showLogin()), showLogin);
