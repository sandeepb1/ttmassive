// Drag handles on the inner edge of the two pinned sidebars. The widths are the CSS variables
// --sb-w and --sb-right-w on #app (see app.css); this only changes them and remembers the result.
// The clamping is pure so node can test it.

import type { Side } from './layout';

export const DEFAULT_W: Record<Side, number> = { left: 300, right: 320 };
export const MIN_W: Record<Side, number> = { left: 220, right: 240 };
export const MAX_W: Record<Side, number> = { left: 560, right: 600 };
/** The conversation never gets narrower than this, whatever the sidebars ask for. */
export const MAIN_MIN_W = 400;
const STEP = 16;
const KEY = 'ttm.sidebar-widths';

export type Widths = Record<Side, number>;

/** `want` limited to the side's own range and to what leaves the conversation its room beside the other sidebar. */
export function clampWidth(side: Side, want: number, other: number, windowW: number): number {
  const room = windowW - other - MAIN_MIN_W;
  const hi = Math.max(MIN_W[side], Math.min(MAX_W[side], room));
  return Math.round(Math.min(hi, Math.max(MIN_W[side], want)));
}

export function parseWidths(raw: string | null): Widths {
  const out: Widths = { ...DEFAULT_W };
  try {
    const o = JSON.parse(raw ?? 'null') as Partial<Record<Side, unknown>> | null;
    for (const side of ['left', 'right'] as const) {
      const v = o?.[side];
      if (typeof v === 'number' && Number.isFinite(v)) out[side] = Math.min(MAX_W[side], Math.max(MIN_W[side], Math.round(v)));
    }
  } catch {
    /* defaults */
  }
  return out;
}

const VAR: Record<Side, string> = { left: '--sb-w', right: '--sb-right-w' };

export function initSidebarResize(app: HTMLElement) {
  let widths: Widths;
  try {
    widths = parseWidths(localStorage.getItem(KEY));
  } catch {
    widths = { ...DEFAULT_W };
  }
  const apply = (side: Side) => {
    if (widths[side] === DEFAULT_W[side]) app.style.removeProperty(VAR[side]);
    else app.style.setProperty(VAR[side], `${widths[side]}px`);
  };
  const save = () => {
    try {
      localStorage.setItem(KEY, JSON.stringify(widths));
    } catch {
      /* not remembered */
    }
  };
  const settle = () => window.dispatchEvent(new Event('ttm-layout-settled'));
  const otherOf = (side: Side): number => {
    const o = side === 'left' ? 'right' : 'left';
    const c = o === 'left' ? 'l' : 'r';
    // An overlay or a collapsed sidebar takes no room beside the conversation.
    return app.classList.contains(`${c}-pinned`) && !app.classList.contains(`${c}-collapsed`) ? widths[o] : 0;
  };
  const set = (side: Side, want: number) => {
    widths[side] = clampWidth(side, want, otherOf(side), window.innerWidth);
    apply(side);
  };
  apply('left');
  apply('right');

  for (const side of ['left', 'right'] as const) {
    const h = document.createElement('div');
    h.className = `sb-resize sb-resize-${side}`;
    h.setAttribute('role', 'separator');
    h.setAttribute('aria-orientation', 'vertical');
    h.setAttribute('aria-label', `Resize the ${side} sidebar`);
    h.tabIndex = 0;
    h.title = 'Drag to resize, double-click to reset';
    app.append(h);

    h.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      h.setPointerCapture(e.pointerId);
      const startX = e.clientX;
      const startW = widths[side];
      // The terminals wait (layout-moving) and fit once at the end; no slide animation while dragging.
      app.classList.add('layout-moving', 'dragging', 'sb-resizing');
      const move = (m: PointerEvent) => set(side, startW + (side === 'left' ? m.clientX - startX : startX - m.clientX));
      const end = () => {
        h.removeEventListener('pointermove', move);
        h.removeEventListener('pointerup', end);
        h.removeEventListener('pointercancel', end);
        app.classList.remove('layout-moving', 'dragging', 'sb-resizing');
        save();
        settle();
      };
      h.addEventListener('pointermove', move);
      h.addEventListener('pointerup', end);
      h.addEventListener('pointercancel', end);
    });
    h.addEventListener('dblclick', () => {
      set(side, DEFAULT_W[side]);
      save();
      settle();
    });
    h.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      e.preventDefault();
      const grow = (e.key === 'ArrowRight') === (side === 'left');
      set(side, widths[side] + (grow ? STEP : -STEP));
      save();
      settle();
    });
  }
  // A narrower window can no longer fit what was asked for.
  window.addEventListener('resize', () => {
    for (const side of ['left', 'right'] as const) {
      const fit = clampWidth(side, widths[side], otherOf(side), window.innerWidth);
      if (fit !== widths[side]) {
        widths[side] = fit;
        apply(side);
      }
    }
  });
}
