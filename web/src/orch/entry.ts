// How the page gets into orchestrator mode. Light on purpose: React and React Flow load the first time it opens.
// The address is `#orch/` or `#orch/<run>`. A session name can never contain a slash, so it cannot collide: a bare
// `#orch` WOULD be a valid session name, and the app would start a session called "orch" from it.

let opened = false;
let mounted = false;
let pushed = false;
let before = '';

const root = () => document.getElementById('orch') as HTMLElement | null;

export const isOrchHash = (h = location.hash) => h.startsWith('#orch/');
export const orchOpen = () => opened;

function runFromHash(): string | null {
  const m = /^#orch\/([A-Za-z0-9_-]+)$/.exec(location.hash);
  return m ? m[1]! : null;
}

function show() {
  const el = root();
  if (!el) return;
  opened = true;
  el.hidden = false;
  // The sessions stay running underneath; they just cannot be reached while this covers them.
  (document.getElementById('app') as (HTMLElement & { inert: boolean }) | null)?.setAttribute('inert', '');
  if (mounted) return;
  mounted = true;
  void import('./mount').then((m) =>
    m.mountOrch(el, {
      initialRun: runFromHash(),
      onExit: exitOrch,
      onRoute: (id) => {
        if (opened) history.replaceState(null, '', `#orch/${id ?? ''}`);
      },
    }),
  );
}

function hide() {
  opened = false;
  const el = root();
  if (el) el.hidden = true;
  document.getElementById('app')?.removeAttribute('inert');
}

/** Make the page match the address. Called when it changes and when the app first appears. */
export function syncOrchRoute() {
  if (isOrchHash() && !opened) show();
  else if (!isOrchHash() && opened) hide();
}

export function openOrch() {
  if (isOrchHash()) return syncOrchRoute();
  before = location.hash;
  history.pushState(null, '', '#orch/');
  pushed = true;
  syncOrchRoute();
}

export function exitOrch() {
  if (pushed) {
    pushed = false;
    history.back(); // the address change closes it
    return;
  }
  history.replaceState(null, '', location.pathname + location.search + before);
  syncOrchRoute();
}
