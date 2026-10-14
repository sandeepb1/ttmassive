// The page's overlay stack, tied to the real browser history. See overlay-stack.ts.

import { createOverlayStack } from './overlay-stack';

const overlays = createOverlayStack(window.history);
window.addEventListener('popstate', () => overlays.popped());

/** Register an open overlay. Call the returned function when it closes for any reason other than Back. */
export const pushOverlay = overlays.push;
