// Touch gestures. Sidebars that slide over the conversation follow your finger from the screen
// edge and snap open or shut on release; bottom sheets can be dragged down to dismiss.

import type { Layout, Side } from './layout';

/**
 * Where a swipe may start to pull a sidebar in, measured from the screen edge. The first few pixels are left alone:
 * the system's own edge gestures (Back, the home bar) live there, and a swipe that starts a little in is the one
 * that is meant for the app.
 */
const EDGE_INSET_PX = 10;
const EDGE_PX = 36;

export const inEdgeZone = (fromEdge: number) => fromEdge >= EDGE_INSET_PX && fromEdge <= EDGE_PX;

/**
 * Pulling a sidebar in from the edge is for the installed app, where nothing else claims the edge. In a browser tab
 * the browser's own Back swipe does, and the menu button opens the list.
 */
function edgeEnabled(): boolean {
  return window.matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true;
}
const SLOP_PX = 8;
/** Faster than this (px/ms) counts as a flick and decides the outcome by itself. */
const FLICK = 0.5;

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

export interface EdgeSwipeOpts {
  app: HTMLElement;
  layout: Layout;
  panels: Record<Side, HTMLElement>;
  scrim: HTMLElement;
  /** A sheet or other modal is over the app: leave the touch alone. */
  blocked: () => boolean;
}

interface Track {
  side: Side;
  /** Opening a closed sidebar, or closing an open one. */
  intent: 'open' | 'close';
  x0: number;
  y0: number;
  t0: number;
  dragging: boolean;
  progress: number;
  lastX: number;
  lastT: number;
  velocity: number;
}

/** Pull a sidebar in from the screen edge, or push an open one away. Only for overlay sidebars. */
export function edgeSwipe(o: EdgeSwipeOpts) {
  let tr: Track | null = null;

  const begin = (e: TouchEvent): Track | null => {
    if (e.touches.length !== 1 || o.blocked()) return null;
    const t = e.touches[0]!;
    const w = window.innerWidth;
    for (const side of ['left', 'right'] as const) {
      if (o.layout.placement(side) !== 'overlay') continue;
      const open = o.layout.isVisible(side);
      const atEdge = edgeEnabled() && (side === 'left' ? inEdgeZone(t.clientX) : inEdgeZone(w - t.clientX));
      if (!open && atEdge) return mk(side, 'open', t);
      if (open) return mk(side, 'close', t);
    }
    return null;
  };

  const mk = (side: Side, intent: 'open' | 'close', t: Touch): Track => ({
    side, intent, x0: t.clientX, y0: t.clientY, t0: performance.now(), dragging: false, progress: intent === 'open' ? 0 : 1,
    lastX: t.clientX, lastT: performance.now(), velocity: 0,
  });

  /** How far the finger has moved toward "open" (negative: toward closed). */
  const towardOpen = (k: Track, clientX: number) => (clientX - k.x0) * (k.side === 'left' ? 1 : -1);

  const paint = (k: Track) => {
    const panel = o.panels[k.side];
    const dir = k.side === 'left' ? -1 : 1;
    panel.style.transform = `translateX(${dir * (1 - k.progress) * 100}%)`;
    panel.style.visibility = 'visible';
    o.scrim.style.opacity = String(k.progress);
    o.scrim.style.visibility = 'visible';
  };

  const clear = (k: Track) => {
    const panel = o.panels[k.side];
    panel.style.transform = '';
    panel.style.visibility = '';
    o.scrim.style.opacity = '';
    o.scrim.style.visibility = '';
    o.app.classList.remove('dragging');
  };

  // The move handler is attached only while a touch that might be a sidebar swipe is down. A touchmove listener on
  // the document that can cancel makes every scroll in the app wait for the main thread, and the terminal is
  // busy while an agent streams; most touches are not swipes, so they now scroll unhindered.
  document.addEventListener(
    'touchstart',
    (e) => {
      tr = begin(e);
      if (tr) document.addEventListener('touchmove', move, { passive: false });
    },
    { passive: true },
  );

  const move = (e: TouchEvent) => {
    const k = tr;
    if (!k) return;
    const t = e.touches[0]!;
    const raw = towardOpen(k, t.clientX);
    const dx = k.intent === 'open' ? raw : -raw; // movement in the direction this gesture wants
    const dyAbs = Math.abs(t.clientY - k.y0);
    if (!k.dragging) {
      if (dyAbs > 12 && dyAbs > Math.abs(dx)) {
        tr = null; // a vertical scroll: not ours
        return;
      }
      if (dx < SLOP_PX || dx < dyAbs * 1.2) return;
      k.dragging = true;
      o.app.classList.add('dragging');
    }
    if (e.cancelable) e.preventDefault();
    const width = o.panels[k.side].offsetWidth || 1;
    k.progress = clamp((k.intent === 'open' ? 0 : 1) + raw / width, 0, 1);
    const now = performance.now();
    const dt = now - k.lastT;
    if (dt > 0) k.velocity = (towardOpen(k, t.clientX) - towardOpen(k, k.lastX)) / dt;
    k.lastX = t.clientX;
    k.lastT = now;
    paint(k);
  };

  const end = () => {
    document.removeEventListener('touchmove', move);
    const k = tr;
    tr = null;
    if (!k || !k.dragging) return;
    // v > 0 means the finger was heading toward "open".
    const open = Math.abs(k.velocity) > FLICK ? k.velocity > 0 : k.progress > 0.5;
    clear(k);
    o.layout.set(k.side, open);
  };
  document.addEventListener('touchend', end);
  document.addEventListener('touchcancel', end);
}

/** Drag a bottom sheet down by its handle to dismiss it; it springs back if you let go early. */
export function dragToDismiss(sheet: HTMLElement, handle: HTMLElement, dismiss: () => void) {
  let y0 = 0;
  let lastY = 0;
  let lastT = 0;
  let v = 0;
  let active = false;

  const set = (dy: number) => {
    sheet.style.transform = `translateY(${dy}px)`;
  };

  handle.addEventListener('pointerdown', (e) => {
    if (e.target instanceof Element && e.target.closest('button')) return; // a tap on ×, not a drag
    active = true;
    y0 = lastY = e.clientY;
    lastT = performance.now();
    v = 0;
    sheet.classList.add('dragging');
    handle.setPointerCapture(e.pointerId);
  });
  handle.addEventListener('pointermove', (e) => {
    if (!active) return;
    const now = performance.now();
    if (now > lastT) v = (e.clientY - lastY) / (now - lastT);
    lastY = e.clientY;
    lastT = now;
    set(Math.max(0, e.clientY - y0));
  });
  const release = (e: PointerEvent) => {
    if (!active) return;
    active = false;
    sheet.classList.remove('dragging');
    const dy = Math.max(0, e.clientY - y0);
    if (dy > 100 || (v > FLICK && dy > 20)) {
      dismiss(); // the sheet's exit animation takes it on from here
    } else {
      sheet.style.transform = '';
    }
  };
  handle.addEventListener('pointerup', release);
  handle.addEventListener('pointercancel', release);
}
