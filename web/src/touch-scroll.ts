// Scrolling the terminal's history with a finger. The terminal draws only the screen (tmux keeps
// the history), so a drag is turned into lines and sent to the server, which scrolls tmux. The
// maths and the rules about keystrokes are here, apart from the DOM, so they can be tested.

/** A finger has to move this far (px) before it is a scroll and not a tap. */
export const SLOP_PX = 8;
/** Below this speed (px per ms) a lifted finger does not carry on. */
export const FLING_MIN = 0.35;
/** The speed at which a fling is no longer worth sending. */
export const FLING_STOP = 0.03;
/** The speed (px per ms) a fling starts at, at most. */
export const FLING_MAX = 6;
/** How much of a fling's speed is left after 16 ms. */
const FRICTION_PER_FRAME = 0.94;
/** Lines sent per request, at most (the server clamps too). */
export const MAX_LINES = 120;

/**
 * Pixels of finger travel become whole lines, and the remainder is kept for the next move so a
 * slow drag still adds up. Dragging down (positive) shows older output, which is up the history.
 */
export class LineCarry {
  private rest = 0;

  add(dy: number, lineHeight: number): number {
    if (!(lineHeight > 0) || !Number.isFinite(dy)) return 0;
    this.rest += dy / lineHeight;
    const whole = Math.trunc(this.rest);
    this.rest -= whole;
    return whole;
  }

  reset() {
    this.rest = 0;
  }
}

export interface Sample {
  y: number;
  t: number;
}

/** Samples kept for the speed: only the last moments of the drag say how fast the finger left. */
const WINDOW_MS = 100;

/** Keep `samples` to the recent ones, with `s` added. */
export function addSample(samples: Sample[], s: Sample): Sample[] {
  return [...samples.filter((x) => s.t - x.t <= WINDOW_MS), s];
}

/** Speed in px per ms (positive: downwards) over the recent samples; 0 when too few. */
export function velocity(samples: Sample[], now: number): number {
  const recent = samples.filter((s) => now - s.t <= WINDOW_MS);
  if (recent.length < 2) return 0;
  const first = recent[0]!;
  const last = recent[recent.length - 1]!;
  const dt = last.t - first.t;
  if (dt <= 0) return 0;
  const v = (last.y - first.y) / dt;
  return Math.sign(v) * Math.min(Math.abs(v), FLING_MAX);
}

/** The speed after `dtMs` of friction. */
export function slow(v: number, dtMs: number): number {
  return v * Math.pow(FRICTION_PER_FRAME, dtMs / 16);
}

/** Whether a lifted finger moving at `v` starts a fling. */
export const flings = (v: number) => Math.abs(v) >= FLING_MIN;

export const keepGoing = (v: number) => Math.abs(v) >= FLING_STOP;

/** At most `MAX_LINES` either way. */
export const clampLines = (n: number) => Math.max(-MAX_LINES, Math.min(MAX_LINES, Math.trunc(n)));

/**
 * What typing does while the history is showing. Anything typed leaves it first, so the keys reach
 * the agent and not tmux's history keys; a lone Escape only leaves it, since sent on it would also
 * interrupt the agent. Escape sequences (arrows, focus and mouse reports, replies to queries) are
 * not typing, and go through as they are.
 */
export function inputWhileScrolled(bytes: Uint8Array): { leave: boolean; forward: boolean } {
  if (bytes.length === 0) return { leave: false, forward: true };
  const first = bytes[0]!;
  if (first === 0x1b) return bytes.length === 1 ? { leave: true, forward: false } : { leave: false, forward: true };
  return { leave: true, forward: true };
}

/** The label of the button that goes back to the live screen. */
export function liveLabel(position: number): string {
  if (position <= 0) return 'Back to live';
  return `Back to live · ${position} line${position === 1 ? '' : 's'} up`;
}
