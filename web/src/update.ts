// A new build is out while the app has been open for days (an installed app is rarely closed). Pages are fetched
// network-first, so a fresh launch always has the newest build; this is for the one that has been running.
//
// - coming back from the background after a while: reload quietly, unless something is half typed;
// - still in use: after a long time, offer it in a toast, never reload under the user's hands.

import { toast } from './ui/toast';
import { buildOf, isStale } from './update-model';

const BACKGROUND_MS = 60_000; // away this long counts as "put down", so a reload costs nothing
const OFFER_AFTER_MS = 30 * 60_000; // in use this long without a reload: offer one
const CHECK_EVERY_MS = 10 * 60_000;

let watching = false;

export function watchForUpdates(host: HTMLElement) {
  if (watching) return; // showApp runs again after every sign-in
  watching = true;
  const running = buildOf(document.documentElement.outerHTML);
  if (!running) return; // dev server
  const loadedAt = Date.now();
  let hiddenAt = 0;
  let offered = false;

  const latest = async () => {
    const r = await fetch('/', { cache: 'no-store' }).catch(() => null);
    return r?.ok ? buildOf(await r.text()) : null;
  };
  /** Text in a box that nobody has sent: reloading would lose it. */
  const halfTyped = () => [...document.querySelectorAll('textarea')].some((t) => t.value.trim() !== '' && !t.classList.contains('xterm-helper-textarea'));

  const offer = () => {
    if (offered) return;
    offered = true;
    toast(host, 'A new version is ready', {
      ms: 600_000,
      action: { label: 'Reload', run: () => location.reload() },
    });
  };

  document.addEventListener('visibilitychange', async () => {
    if (document.hidden) {
      hiddenAt = Date.now();
      return;
    }
    const away = hiddenAt ? Date.now() - hiddenAt : 0;
    hiddenAt = 0;
    if (!isStale(running, await latest())) return;
    if (away >= BACKGROUND_MS && !halfTyped()) location.reload();
    else if (Date.now() - loadedAt >= OFFER_AFTER_MS) offer();
  });

  window.setInterval(async () => {
    if (document.hidden || Date.now() - loadedAt < OFFER_AFTER_MS) return;
    if (isStale(running, await latest())) offer();
  }, CHECK_EVERY_MS);
}
