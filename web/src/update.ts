// A new version is out while the app has been open for days (an installed app is rarely closed). Two kinds:
//
// - the page is behind the server (it was updated, or a deploy landed). Pages are fetched network-first, so a fresh
//   launch always has the newest build: coming back after a while reloads quietly unless something is half typed,
//   otherwise a toast offers the reload;
// - the server is behind a newer GitHub release (server-update.ts): a toast offers to install it.
//
// Both are checked on launch and whenever the app comes back to the foreground, nothing in between.

import { fetchUpdate, offerServerUpdate } from './server-update';
import { failureNews, shouldOffer } from './server-update-model';
import { toast } from './ui/toast';
import { buildOf, isStale } from './update-model';

const BACKGROUND_MS = 60_000; // away this long counts as "put down", so a reload costs nothing
const SEEN_KEY = 'ttm.update.failureSeen';

function seenAt(): number {
  try {
    return Number(localStorage.getItem(SEEN_KEY)) || 0;
  } catch {
    return 0;
  }
}

let watching = false;

export function watchForUpdates(host: HTMLElement) {
  if (watching) return; // showApp runs again after every sign-in
  watching = true;
  const running = buildOf(document.documentElement.outerHTML); // null on a dev server
  let hiddenAt = 0;
  let checking = false;
  let reloadOffered = false;
  let offered: string | null = null;

  const latestBuild = async () => {
    const r = await fetch('/', { cache: 'no-store' }).catch(() => null);
    return r?.ok ? buildOf(await r.text()) : null;
  };
  /** Text in a box that nobody has sent: reloading would lose it. */
  const halfTyped = () => [...document.querySelectorAll('textarea')].some((t) => t.value.trim() !== '' && !t.classList.contains('xterm-helper-textarea'));

  const check = async (away: number) => {
    if (checking) return;
    checking = true;
    try {
      if (running && isStale(running, await latestBuild())) {
        if (away >= BACKGROUND_MS && !halfTyped()) {
          location.reload();
        } else if (!reloadOffered) {
          reloadOffered = true;
          toast(host, 'A new version is ready', { ms: 600_000, action: { label: 'Reload', run: () => location.reload() } });
        }
        return; // reloading comes first; the server's own update is offered afterwards if it is still news
      }
      const u = await fetchUpdate();
      if (!u) return;
      const failed = failureNews(u.lastResult, seenAt());
      if (failed && u.lastResult) {
        try {
          localStorage.setItem(SEEN_KEY, String(u.lastResult.at));
        } catch {
          // Without storage it is told again next launch.
        }
        toast(host, failed, { ms: 15_000 });
      } else if (shouldOffer(u, offered)) {
        offered = u.latest;
        offerServerUpdate(host, u);
      }
    } finally {
      checking = false;
    }
  };

  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      hiddenAt = Date.now();
      return;
    }
    const away = hiddenAt ? Date.now() - hiddenAt : 0;
    hiddenAt = 0;
    void check(away);
  });
  void check(0);
}
