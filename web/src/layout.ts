// Where the two sidebars go. At 1100px or wider both sit beside the conversation; on a tablet in
// portrait only the session list does; on a phone both slide over it. The decision is pure so node
// can test it; `Layout` below applies it to the page.

export type Side = 'left' | 'right';
export type Placement = 'pinned' | 'overlay';

export const PIN_LEFT_MIN_W = 700;
export const PIN_LEFT_MIN_H = 480;
export const PIN_RIGHT_MIN_W = 1100;

export function placement(side: Side, width: number, height: number): Placement {
  if (side === 'left') return width >= PIN_LEFT_MIN_W && height >= PIN_LEFT_MIN_H ? 'pinned' : 'overlay';
  return width >= PIN_RIGHT_MIN_W && height >= PIN_LEFT_MIN_H ? 'pinned' : 'overlay';
}

export interface SideState {
  /** Pinned sidebars: hidden by the user. Remembered. */
  collapsed: boolean;
  /** Overlay sidebars: on screen right now. Not remembered. */
  open: boolean;
}

export type LayoutState = Record<Side, SideState>;

export const INITIAL: LayoutState = {
  left: { collapsed: false, open: false },
  right: { collapsed: false, open: false },
};

export function visible(state: LayoutState, side: Side, width: number, height: number): boolean {
  return placement(side, width, height) === 'pinned' ? !state[side].collapsed : state[side].open;
}

/** The state after the user taps a sidebar's button. Opening one overlay closes the other. */
export function toggled(state: LayoutState, side: Side, width: number, height: number): LayoutState {
  const other: Side = side === 'left' ? 'right' : 'left';
  const next: LayoutState = { left: { ...state.left }, right: { ...state.right } };
  if (placement(side, width, height) === 'pinned') {
    next[side].collapsed = !next[side].collapsed;
  } else {
    next[side].open = !next[side].open;
    if (next[side].open) next[other].open = false;
  }
  return next;
}

/** Set a side open or closed (a swipe, a tap on the scrim, picking a session). */
export function setOpen(state: LayoutState, side: Side, open: boolean, width: number, height: number): LayoutState {
  if (visible(state, side, width, height) === open) return state;
  return toggled(state, side, width, height);
}

/** Close every overlay, leaving pinned sidebars alone. */
export function closeOverlays(state: LayoutState): LayoutState {
  return { left: { ...state.left, open: false }, right: { ...state.right, open: false } };
}

// ---- the page ------------------------------------------------------------------------------

const KEY = 'ttm.sidebars';
/** How long a pinned sidebar takes to slide (the transition in app.css). */
const SLIDE_MS = 260;

function load(): LayoutState {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? 'null') as Partial<Record<Side, { collapsed?: boolean }>> | null;
    return {
      left: { collapsed: raw?.left?.collapsed === true, open: false },
      right: { collapsed: raw?.right?.collapsed === true, open: false },
    };
  } catch {
    return INITIAL;
  }
}

/** Keeps the layout classes on `#app` in step with the window and the user's choices. */
export class Layout {
  private state: LayoutState = load();
  private listeners = new Set<() => void>();
  private settleTimer = 0;

  private app: HTMLElement;

  constructor(app: HTMLElement) {
    this.app = app;
    window.addEventListener('resize', () => this.apply());
    window.addEventListener('orientationchange', () => this.apply());
    this.apply();
  }

  private get w() {
    return window.innerWidth;
  }
  private get h() {
    return window.innerHeight;
  }

  placement(side: Side): Placement {
    return placement(side, this.w, this.h);
  }

  isVisible(side: Side): boolean {
    return visible(this.state, side, this.w, this.h);
  }

  toggle(side: Side) {
    this.update(toggled(this.state, side, this.w, this.h));
  }

  set(side: Side, open: boolean) {
    this.update(setOpen(this.state, side, open, this.w, this.h));
  }

  /** A choice was made inside an overlay (a session picked): get it out of the way. */
  dismissOverlays() {
    this.update(closeOverlays(this.state));
  }

  onChange(cb: () => void): () => void {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  private update(next: LayoutState) {
    this.state = next;
    try {
      localStorage.setItem(
        KEY,
        JSON.stringify({ left: { collapsed: next.left.collapsed }, right: { collapsed: next.right.collapsed } }),
      );
    } catch {
      /* the choice just isn't remembered */
    }
    this.apply();
  }

  private apply() {
    const slides = this.slidesNow();
    for (const side of ['left', 'right'] as const) {
      const p = this.placement(side);
      const c = side === 'left' ? 'l' : 'r';
      this.app.classList.toggle(`${c}-pinned`, p === 'pinned');
      this.app.classList.toggle(`${c}-collapsed`, p === 'pinned' && this.state[side].collapsed);
      this.app.classList.toggle(`${c}-open`, p === 'overlay' && this.state[side].open);
    }
    if (slides) this.holdRefit();
    for (const cb of this.listeners) cb();
  }

  /** Whether applying the state now slides a pinned sidebar in or out (it was pinned, and still is, with another collapsed flag). */
  private slidesNow(): boolean {
    if (this.app.hidden) return false;
    return (['left', 'right'] as const).some((side) => {
      const c = side === 'left' ? 'l' : 'r';
      const was = this.app.classList.contains(`${c}-collapsed`);
      return this.app.classList.contains(`${c}-pinned`) && this.placement(side) === 'pinned' && was !== this.state[side].collapsed;
    });
  }

  /**
   * While a sidebar slides, the conversation's width changes every frame. Terminals must not refit on each one (it
   * reflows the whole screen and tells tmux), so they wait for `ttm-layout-settled` and fit once at the end.
   */
  private holdRefit() {
    this.app.classList.add('layout-moving');
    window.clearTimeout(this.settleTimer);
    this.settleTimer = window.setTimeout(() => {
      this.app.classList.remove('layout-moving');
      window.dispatchEvent(new Event('ttm-layout-settled'));
    }, SLIDE_MS + 60);
  }
}
