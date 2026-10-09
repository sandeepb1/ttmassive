import '@fontsource-variable/inter/wght.css';
import '@fontsource-variable/jetbrains-mono/wght.css';
import './style.css';
import { initTheme } from './theme';
import { TerminalSession } from './term';
import { SessionDrawer } from './sessions';
import { PromptBar, StoppedBar } from './bars';
import { UsageView } from './usage';
import { ChatView } from './chat';
import { chooseView } from './chat-model';
import type { Analysis } from './protocol';

initTheme();

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
let session: TerminalSession | null = null;
let drawer: SessionDrawer | null = null;
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
  $('app').hidden = true;
  $('login').hidden = false;
  $<HTMLInputElement>('pw').focus();
}

let connMsg = '';
let activityMsg = '';
function renderStatus() {
  $('status').textContent = connMsg || activityMsg;
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
  const thisChat = new ChatView($('chat'), name, {
    onUnauthorized: showLogin,
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
  usage ??= new UsageView($('usage'), $('app'), showLogin);
  drawer ??= new SessionDrawer($('app'), () => current, openSession, showLogin, () => void logout(), (id) =>
    usage?.tokensLabel(id),
  );
  usage.start();
  openSession(sessionName());
}

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

for (const [id, delta] of [['font-dec', -1], ['font-inc', 1]] as const) {
  $(id).addEventListener('pointerdown', (e) => {
    e.preventDefault(); // keep the soft keyboard up
    session?.bumpFont(delta);
    session?.focus();
  });
}

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
$('menu-btn').addEventListener('pointerdown', (e) => {
  e.preventDefault(); // don't steal focus from the terminal
  usage?.hide();
  drawer?.toggle();
});
$('usage').addEventListener('pointerdown', (e) => {
  e.preventDefault();
  drawer?.hide();
  usage?.toggle();
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
    if (d?.type === 'usage' && !$('app').hidden) {
      drawer?.hide();
      usage?.show();
    }
  });
  window.addEventListener('load', () => navigator.serviceWorker.register('/sw.js').catch(() => {}));
}

fetch('/api/me').then((r) => (r.ok ? showApp() : showLogin()), showLogin);
