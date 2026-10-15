// The page's overlay stack, tied to the real browser history. See overlay-stack.ts.

import { createOverlayStack } from './overlay-stack';

const overlays = createOverlayStack(window.history);

let traversing = false;
let restore: (() => void) | null = null;

window.addEventListener('popstate', () => {
  if (!overlays.popped()) return;
  // Going back moved the address to the entry from before the overlay opened. If a session was opened while it was
  // up (a pick from the sidebar, a new session from the sheet) that entry names the old one, and the page's own
  // `hashchange` handler would switch back to it. This traversal is ours, not navigation: say so until the browser has
  // delivered its `hashchange` (it follows `popstate` in the same task), then put the address right.
  traversing = true;
  setTimeout(() => {
    traversing = false;
    restore?.();
  }, 0);
});

/** Register an open overlay. Call the returned function when it closes for any reason other than Back. */
export const pushOverlay = overlays.push;

/** True while an address change is only the overlay history entry going away: not a request to open another session. */
export const isOverlayTraversal = () => traversing;

/** What to do afterwards to make the address name what the page shows. */
export function onAddressRestore(cb: () => void) {
  restore = cb;
}
