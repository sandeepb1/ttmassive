import '@fontsource-variable/inter/wght.css';
import '@fontsource-variable/jetbrains-mono/wght.css';
import './style.css';
import { initTheme } from './theme';
import { initUiScale } from './scale';
import { TerminalSession } from './term';
import { SessionSidebar } from './sessions';
import { PromptBar, StoppedBar } from './bars';
import { UsageView } from './usage';
import { ChatView } from './chat';
import { Layout } from './layout';
import { edgeSwipe } from './gestures';
import { RightBar } from './rightbar';
import { SettingsSheet } from './settings';
import { icon } from './icons';
import { chooseView } from './chat-model';
import type { Analysis } from './protocol';

initTheme();
initUiScale();

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const layout = new Layout($('app'));
let session: TerminalSession | null = null;
let sidebar: SessionSidebar | null = null;
let rightbar: RightBar | null = null;
let settings: SettingsSheet | null = null;
let usage: UsageView | null = null;
let chat: ChatView | null = null;
let view: 'chat' | 'term' = 'term';
/** Whether this session's starting view has been picked yet. */
let viewChosen = false;
let current = '';

function sessionName(): string {
  const n = decodeURIComponent(location.hash.slice(1));
  return /^[A-Za-z0-9_-]{1,64}$/.test(n) ? n : 'main';
}

const viewKey = (name: string) => `ttm.view.${name}`;

function savedView(name: string): string | null {
  try {
    return localStorage.getItem(viewKey(name));
  } catch {
    return null;
  }
}

/** Show the chat or the terminal for the current session. `remember` keeps it for next time. */
function setView(next: 'chat' | 'term', remember: boolean) {
  view = next;
  $('app').classList.toggle('chat-mode', next === 'chat');
  for (const b of $('viewtoggle').querySelectorAll('button')) {
    b.setAttribute('aria-pressed', String(b.dataset.view === next));
  }
  if (next === 'chat') {
    chat?.show();
  } else {
    chat?.hide();
    session?.focus();
  }
  if (remember) {
    try {
      localStorage.setItem(viewKey(current), next);
    } catch {
      /* the choice just isn't remembered */
    }
  }
}

function showLogin() {
  session?.dispose();
  session = null;
  chat?.dispose();
  chat = null;
  usage?.stop();
  sidebar?.setActive(false);
  layout.dismissOverlays();
  $('app').hidden = true;
  $('login').hidden = false;
  $<HTMLInputElement>('pw').focus();
}

let connMsg = '';
let activityMsg = '';
function renderStatus() {
  const text = connMsg || activityMsg;
  $('status').textContent = text;
  rightbar?.setStatus(text);
}

const ACTIVITY: Record<Analysis['activity'], string> = {
  working: 'working…',
  waiting: 'waiting for you',
  idle: '',
  unknown: '',
};

let promptBar: PromptBar | null = null;
let stoppedBar: StoppedBar | null = null;

function openSession(name: string) {
  session?.dispose();
  chat?.dispose();
  current = name;
  viewChosen = false;
  $('viewtoggle').hidden = true;
  setView('term', false);
  $('title-name').textContent = name;
  rightbar?.setSession(name);
  sidebar?.reload();
  const thisChat: ChatView = new ChatView($('chat'), name, {
    onUnauthorized: showLogin,
    sheetHost: $('main'),
    onData: (data) => {
      if (chat === thisChat) rightbar?.setData(data);
    },
    onInfo: (info) => {
      if (chat !== thisChat) return;
      rightbar?.setInfo(info, {
        model: () => thisChat.openModelPicker(),
        mode: () => thisChat.openModeSheet(),
        more: () => thisChat.openMoreSheet(),
        resume: () => thisChat.openResumeSheet(),
      });
    },
    onEligible: (eligible) => {
      if (chat !== thisChat) return; // a session we have already left
      $('viewtoggle').hidden = !eligible;
      if (!eligible) return setView('term', false);
      if (!viewChosen) {
        viewChosen = true;
        setView(chooseView(savedView(name), true), false);
      }
    },
  });
  chat = thisChat;
  void thisChat.probe();
  if (location.hash.slice(1) !== name) history.replaceState(null, '', `#${encodeURIComponent(name)}`);
  connMsg = '';
  activityMsg = '';
  renderStatus();
  const sess = new TerminalSession(
    $('term'),
    name,
    (s) => {
      connMsg = s;
      renderStatus();
    },
    showLogin,
    $('keybar'),
    {
      onStopped: (resumable) => stoppedBar?.show(resumable),
      onScreen: (a) => {
        promptBar?.render(a);
        activityMsg = a ? ACTIVITY[a.activity] : '';
        renderStatus();
      },
    },
  );
  session = sess;
  promptBar = new PromptBar($('prompt-bar'), (choice, id) => sess.answer(choice, id));
  stoppedBar = new StoppedBar($('stopped-bar'), async (fresh) => {
    const r = await fetch(`/api/sessions/${encodeURIComponent(name)}/restore`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ fresh }),
    }).catch(() => null);
    if (!r) return 'Server unreachable';
    if (!r.ok) return await r.text();
    stoppedBar?.hide();
    sess.reconnect();
    return null;
  });
  promptBar.render(null);
  stoppedBar.hide();
  sess.connect();
  sess.focus();
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
    () => current,
    openSession,
    showLogin,
    { navigated: () => layout.dismissOverlays(), collapse: () => layout.toggle('left'), openSettings },
    (id) => usage?.tokensLabel(id),
  );
  rightbar ??= new RightBar($('right'), usage.body, {
    collapse: () => layout.toggle('right'),
    refreshUsage: () => usage?.refreshNow(),
    search: (q) => chat?.search(q) ?? [],
    jump: showMessage,
  });
  usage.start(); // before syncPanels: start() resets what the sidebar asked the usage view to watch
  syncPanels();
  openSession(sessionName());
}

/** Bring a message found by search into view in the chat, switching to the chat if needed. */
function showMessage(id: string) {
  const go = () => chat?.jumpTo(id);
  layout.dismissOverlays(); // on a phone the sidebar would cover it
  if (view === 'chat') return void go();
  setView('chat', true);
  window.setTimeout(go, 700); // the chat scrolls to the end once it has loaded: land after that
}

function openSettings() {
  layout.dismissOverlays(); // a sheet would sit under an open sidebar
  settings ??= new SettingsSheet($('app'), {
    onUnauthorized: showLogin,
    logout: () => void logout(),
    currentSession: () => current,
    terminalFont: () => session?.font ?? null,
    setTerminalFont: (px) => session?.setFont(px),
  });
  settings.open();
}

/** Poll and refresh only what is on screen, and keep hidden sidebars out of the tab order. */
function syncPanels() {
  const left = layout.isVisible('left');
  const right = layout.isVisible('right');
  sidebar?.setActive(left);
  usage?.watch(right && !$('app').hidden);
  rightbar?.setActive(right && !$('app').hidden);
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

document.addEventListener('visibilitychange', () => {
  if (!document.hidden) {
    session?.nudge();
    usage?.nudge();
    chat?.nudge();
  }
});
window.addEventListener('online', () => {
  session?.nudge();
  usage?.nudge();
  chat?.nudge();
});
for (const b of $('viewtoggle').querySelectorAll('button')) {
  b.addEventListener('pointerdown', (e) => e.preventDefault()); // don't close the keyboard
  b.addEventListener('click', () => setView(b.dataset.view === 'chat' ? 'chat' : 'term', true));
}
window.addEventListener('hashchange', () => {
  if (sessionName() !== current && !$('app').hidden) openSession(sessionName());
});
// Sidebar buttons: pointerdown + preventDefault so the terminal keeps focus and the keyboard stays up.
$('menu-btn').append(icon('menu'));
$('panel-btn').append(icon('panel-right'));
for (const [id, side] of [['menu-btn', 'left'], ['panel-btn', 'right']] as const) {
  $(id).addEventListener('pointerdown', (e) => e.preventDefault());
  $(id).addEventListener('click', () => layout.toggle(side));
}
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
