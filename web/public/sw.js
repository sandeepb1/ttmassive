// App-shell cache. Terminal traffic (/api, /ws) is never cached.
const CACHE = 'ttm-shell-v4';
const SHELL = ['/', '/manifest.webmanifest', '/icon.svg'];

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== 'GET' || url.origin !== location.origin) return;
  if (url.pathname.startsWith('/api/') || url.pathname === '/ws') return;

  if (req.mode === 'navigate') {
    // Network first so a new build is picked up; fall back to the cached shell offline.
    e.respondWith(
      fetch(req)
        .then((res) => {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put('/', copy));
          return res;
        })
        .catch(() => caches.match('/')),
    );
    return;
  }

  // Hashed build assets are immutable: cache first.
  e.respondWith(
    caches.match(req).then(
      (hit) =>
        hit ||
        fetch(req).then((res) => {
          if (res.ok) {
            const copy = res.clone();
            caches.open(CACHE).then((c) => c.put(req, copy));
          }
          return res;
        }),
    ),
  );
});

// ---- Web Push -------------------------------------------------------------

// Payload: { title, body, session }. Skip the popup when you are already looking
// at that session; otherwise show one notification per session.
self.addEventListener('push', (e) => {
  let data = {};
  try {
    data = e.data ? e.data.json() : {};
  } catch {
    data = { title: 'ttm', body: e.data ? e.data.text() : '' };
  }
  const session = data.session || 'main';
  // A usage-limit warning is not about a session: always show it, and open the usage panel.
  if (data.limit) {
    e.waitUntil(
      self.registration.showNotification(data.title || 'Usage limit', {
        body: data.body || '',
        tag: session, // one per tool, so a newer warning replaces an older one
        renotify: true,
        icon: '/icon-192.png',
        badge: '/icon-192.png',
        data: { limit: true },
      }),
    );
    return;
  }
  e.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      const watching = wins.some(
        (w) =>
          w.visibilityState === 'visible' &&
          w.focused &&
          (new URL(w.url).hash.slice(1) || 'main') === decodeURIComponent(session),
      );
      if (watching && !/test/i.test(data.title || '')) return;
      await self.registration.showNotification(data.title || 'ttm', {
        body: data.body || '',
        tag: session,
        renotify: true,
        icon: '/icon-192.png',
        badge: '/icon-192.png',
        // Buttons for the question the agent is asking, e.g. "Yes" / "No, and tell Claude...".
        actions: Array.isArray(data.actions) ? data.actions.slice(0, 2) : [],
        data: { session, prompt_id: data.prompt_id || null },
      });
    })(),
  );
});

// Answer a question straight from the notification, without opening the app. The prompt id
// makes the server refuse if the screen has moved on, so a stale button can't answer anything.
async function answerFromNotification(session, choice, promptId) {
  let body = 'Could not reach the server';
  let ok = false;
  try {
    const r = await fetch(`/api/sessions/${encodeURIComponent(session)}/answer`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ choice, prompt_id: promptId }),
    });
    ok = r.ok;
    body = r.ok ? `Answered: ${(await r.json()).sent || 'sent'}` : r.status === 409 ? 'The question changed. Open ttm to check.' : 'Could not answer. Open ttm.';
  } catch {
    /* keep the offline message */
  }
  await self.registration.showNotification(ok ? `${session}: sent` : `${session}: not sent`, {
    body,
    tag: session,
    icon: '/icon-192.png',
    data: { session },
  });
}

self.addEventListener('notificationclick', (e) => {
  e.notification.close();
  const data = e.notification.data || {};
  const session = data.session || 'main';
  if (data.limit) {
    e.waitUntil(
      (async () => {
        const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
        if (wins[0]) {
          await wins[0].focus();
          wins[0].postMessage({ type: 'usage' });
        } else {
          await self.clients.openWindow('/');
        }
      })(),
    );
    return;
  }
  if (e.action && e.action.startsWith('choice:')) {
    e.waitUntil(answerFromNotification(session, e.action.slice('choice:'.length), data.prompt_id || null));
    return;
  }
  e.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      const win = wins[0];
      if (win) {
        await win.focus();
        win.postMessage({ type: 'open', session });
      } else {
        await self.clients.openWindow('/#' + encodeURIComponent(session));
      }
    })(),
  );
});
