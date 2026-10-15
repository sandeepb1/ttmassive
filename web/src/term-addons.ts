// Pure decisions behind the terminal addons (progress bar, link opening), kept free of xterm and
// the DOM so node can test them. `term.ts` does the wiring.

/** OSC 9;4 progress as the addon reports it: 0 off, 1 normal, 2 error, 3 indeterminate, 4 paused. */
export interface Progress {
  state: 0 | 1 | 2 | 3 | 4;
  value: number;
}

export type ProgressKind = 'normal' | 'error' | 'indeterminate' | 'paused';

export interface ProgressView {
  kind: ProgressKind;
  /** 0-100, or null when the bar has no length to show (indeterminate). */
  pct: number | null;
}

/** What the bar should look like, or null when it should be hidden. */
export function progressView(p: Progress): ProgressView | null {
  const pct = Math.min(100, Math.max(0, Math.round(p.value)));
  switch (p.state) {
    case 1:
      return { kind: 'normal', pct };
    case 2:
      // An error with no value still needs a visible bar.
      return { kind: 'error', pct: p.value > 0 ? pct : 100 };
    case 3:
      return { kind: 'indeterminate', pct: null };
    case 4:
      return { kind: 'paused', pct: p.value > 0 ? pct : 100 };
    default:
      return null;
  }
}

export type LinkAction = 'open' | 'confirm' | 'block';

const looksLikeHost = (s: string) => /^(?:[a-z][a-z0-9+.-]*:\/\/)?[a-z0-9-]+(?:\.[a-z0-9-]+)+(?:[/:?#]|$)/i.test(s);

function hostOf(s: string): string | null {
  try {
    return new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(s) ? s : `https://${s}`).hostname.toLowerCase();
  } catch {
    return null;
  }
}

/**
 * What to do when someone taps a link. Plain URLs found in the output always open. An OSC 8 link
 * can show any text over any target, so only http(s) and mailto are allowed, and a label that
 * looks like an address but points at another host is confirmed first (the phishing case).
 * Ordinary labels such as "PR #12" open directly.
 */
export function linkAction(text: string, uri: string): LinkAction {
  let u: URL;
  try {
    u = new URL(uri);
  } catch {
    return 'block';
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:' && u.protocol !== 'mailto:') return 'block';
  const label = text.trim();
  if (u.protocol === 'mailto:' || !looksLikeHost(label)) return 'open';
  const shown = hostOf(label);
  const real = u.hostname.toLowerCase();
  return shown === real ? 'open' : 'confirm';
}

/**
 * A program's window title (OSC 0/2) as a header subtitle: control characters and the spinner or
 * star glyph agents put in front are dropped, spaces collapse, and a long title is cut short.
 */
export function cleanTitle(raw: string, max = 60): string {
  const plain = raw.replace(/\s+/g, ' ').replace(/\p{C}/gu, '').trim().replace(/^[^\p{L}\p{N}]+/u, '');
  return plain.length > max ? `${plain.slice(0, max - 1).trimEnd()}…` : plain;
}

/** Whether a bell is worth a dot, a sound and a buzz: not for the pane you are looking at, and not twice in `gapMs`. */
export function bellRings(o: { now: number; last: number; focused: boolean; hidden: boolean; gapMs: number }): boolean {
  if (o.focused && !o.hidden) return false;
  return o.now - o.last >= o.gapMs;
}

/**
 * Whether the app may move focus into the terminal by itself. On a touch screen that raises the soft keyboard, which
 * covers half the screen: it must not happen because a session opened, a header button was tapped or the menu was
 * opened. It is fine when the keyboard is already up (switching session mid-typing keeps typing). A tap on the
 * terminal itself focuses it natively, so the person can always ask for the keyboard.
 */
export function mayFocusTerminal(o: { coarsePointer: boolean; keyboardUp: boolean }): boolean {
  return !o.coarsePointer || o.keyboardUp;
}
