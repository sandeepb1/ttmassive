import { Terminal, type ILink } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { ClipboardAddon, type IClipboardProvider } from '@xterm/addon-clipboard';
import { ImageAddon } from '@xterm/addon-image';
import { ProgressAddon } from '@xterm/addon-progress';
import { UnicodeGraphemesAddon } from '@xterm/addon-unicode-graphemes';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { el } from './dom';
import { KeyBar } from './keybar';
import type { Analysis, ClientMsg, ServerMsg } from './protocol';
import { MONO_FALLBACK, MONO_FONT, onThemeChange, xtermTheme } from './theme';
import { cleanTitle, linkAction, progressView, type Progress } from './term-addons';
import { skewFromDate } from './activity-chip';
import { findPaths } from './paths';
import { markedRows } from './turn-marks';
import { newlineFor } from './keys';
import {
  LineCarry,
  SLOP_PX,
  addSample,
  clampLines,
  flings,
  inputWhileScrolled,
  keepGoing,
  liveLabel,
  slow,
  velocity,
  type Sample,
} from './touch-scroll';
import { filesOf, pathsToPaste } from './term-drop';
import { UnauthorizedError, uploadFile } from './term-upload';
import { getOptionMeta, onPrefsChange } from './prefs';

const PING_MS = 20_000;
const SCREEN_POLL_MS = 2000;
/** Scrolls are sent in batches this often, one at a time. */
const SCROLL_BATCH_MS = 30;
/** A reply that never came: send again after this long. */
const SCROLL_LOST_MS = 1500;

export interface SessionHooks {
  /** The session exists only as saved output; offer a restore. */
  onStopped: (resumable: boolean) => void;
  /** Latest screen analysis, or null when unavailable. */
  onScreen: (a: Analysis | null, skewMs: number) => void;
  /** The program reported progress (OSC 9;4). `kind` is null when it cleared it. */
  onProgress: (kind: 'normal' | 'error' | 'indeterminate' | 'paused' | null) => void;
  /** A file path in the output was tapped. */
  onOpenFile: (path: string, line: number | null) => void;
  /** Which agent the session runs, or null for a plain shell. */
  agent: () => 'claude' | 'codex' | null;
  /** The program set its window title (OSC 0/2), already cleaned for display. */
  onTitle: (title: string) => void;
  /** The program rang the bell. */
  onBell: () => void;
  /** The key bar's New button was tapped: offer to start a session beside `anchor`. */
  onLaunch: (anchor: HTMLElement) => void;
  /** A long press on the terminal asked for its text as selectable text. */
  onSelectText: () => void;
}

/** Copies text for a program that asked via OSC 52. Reading the clipboard is never allowed. */
class WriteOnlyClipboard implements IClipboardProvider {
  constructor(private blocked: (text: string) => void) {}

  readText(): string {
    return '';
  }

  async writeText(_selection: string, text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Safari only allows this inside a tap; hand the text to a button the user can press.
      this.blocked(text);
    }
  }
}
const MAX_BACKOFF_MS = 10_000;

const FONT_KEY = 'ttm.fontSize';
const FONT_MIN = 9;
const FONT_MAX = 32;

function initialFontSize(): number {
  try {
    const saved = Number(localStorage.getItem(FONT_KEY));
    if (saved >= FONT_MIN && saved <= FONT_MAX) return saved;
  } catch {
    /* storage unavailable (private mode): fall through to the default */
  }
  // Big tablet screens read better with a larger default than phones.
  return window.innerWidth >= 900 ? 17 : 14;
}

export class TerminalSession {
  private term = new Terminal({
    cursorBlink: true,
    fontSize: initialFontSize(),
    scrollback: 0, // tmux owns scrollback
    allowProposedApi: true,
    macOptionIsMeta: getOptionMeta(),
    fontFamily: MONO_FALLBACK, // swapped to MONO_FONT once the web font has loaded
    theme: xtermTheme(),
    // OSC 8 hyperlinks: only http(s)/mailto, and a label that fakes another address is confirmed.
    linkHandler: {
      allowNonHttpProtocols: false,
      activate: (_e, uri, range) => {
        const label =
          this.term.buffer.active.getLine(range.start.y - 1)?.translateToString(true, range.start.x - 1, range.end.x) ?? '';
        this.openLink(label, uri);
      },
    },
  });
  private toastEl: HTMLElement | null = null;
  private toastTimer = 0;
  private barEl: HTMLElement | null = null;
  private gl: WebglAddon | null = null;
  private turnSnippets: string[] = [];
  private marksEl: HTMLElement | null = null;
  private marksTimer = 0;
  private fit = new FitAddon();
  private ws: WebSocket | null = null;
  private attempt = 0;
  private closed = false;
  private stopped = false;
  private ended = false;
  private screenTimer = 0;
  private pingTimer = 0;
  private retryTimer = 0;
  private enc = new TextEncoder();
  private keybar: KeyBar;
  /** The history is showing (tmux's copy-mode), this many lines up. Null: the live screen. */
  private scrolled: { position: number } | null = null;
  private scrollPending = 0;
  private scrollTimer = 0;
  /** When the scroll now awaiting its reply was sent; 0 when none is. */
  private scrollSent = 0;
  private flingFrame = 0;
  private liveEl: HTMLButtonElement | null = null;
  private cleanup: () => void = () => {};

  constructor(
    private el: HTMLElement,
    private session: string,
    private onStatus: (s: string) => void,
    private onUnauthorized: () => void,
    keybarEl: HTMLElement,
    private hooks: SessionHooks,
    gpu = true,
  ) {
    this.keybar = new KeyBar(
      keybarEl,
      (b) => this.sendInput(b),
      () => this.term.modes.applicationCursorKeysMode,
      () => this.term.focus(),
      (anchor) => this.hooks.onLaunch(anchor),
    );
    this.term.loadAddon(this.fit);
    this.term.open(el);
    this.setGpu(gpu);
    this.loadAddons();
    this.watchTaps();
    this.term.attachCustomKeyEventHandler((e) => this.newlineKey(e));
    this.watchDrops();
    this.watchLongPress();
    this.watchTouchScroll();
    this.term.onTitleChange((t) => this.hooks.onTitle(cleanTitle(t)));
    this.term.onBell(() => this.hooks.onBell());
    this.term.onData((d) => this.sendInput(this.enc.encode(this.keybar.transform(d))));
    this.term.onBinary((d) => this.sendInput(Uint8Array.from(d, (c) => c.charCodeAt(0))));
    this.term.onResize(({ rows, cols }) => this.sendCtl({ type: 'resize', rows, cols }));
    this.term.onWriteParsed(() => this.scheduleMarks());
    this.term.onResize(() => this.scheduleMarks());

    // Orientation change, soft keyboard and window resizes all land here.
    const refit = () => requestAnimationFrame(() => this.fit.fit());
    const ro = new ResizeObserver(refit);
    ro.observe(el);
    window.addEventListener('orientationchange', refit);
    window.visualViewport?.addEventListener('resize', refit);
    // Restyle live when the theme changes, and re-measure once the web font has actually loaded.
    const offTheme = onThemeChange(() => {
      this.term.options.theme = xtermTheme();
    });
    void document.fonts?.load("14px 'JetBrains Mono Variable'").then(() => {
      if (this.closed) return;
      this.term.options.fontFamily = MONO_FONT;
      this.fit.fit();
    });
    const offPrefs = onPrefsChange(() => {
      this.term.options.macOptionIsMeta = getOptionMeta();
    });
    this.cleanup = () => {
      offTheme();
      offPrefs();
      ro.disconnect();
      window.removeEventListener('orientationchange', refit);
      window.visualViewport?.removeEventListener('resize', refit);
    };
    this.fit.fit();
  }

  /**
   * Draw on the GPU. The default DOM renderer is the main source of lag when an agent streams a
   * lot of output, but browsers allow only a handful of WebGL contexts (fewer on a tablet), so
   * with many panes open only the busiest ones get one. Without WebGL, or if the context is lost,
   * xterm keeps its DOM renderer.
   */
  setGpu(on: boolean) {
    if (on === (this.gl !== null) || this.closed) return;
    if (!on) {
      this.gl?.dispose();
      this.gl = null;
      return;
    }
    try {
      const gl = new WebglAddon();
      gl.onContextLoss(() => {
        gl.dispose();
        if (this.gl === gl) this.gl = null;
      });
      this.term.loadAddon(gl);
      this.gl = gl;
    } catch {
      /* no WebGL here: the DOM renderer stays */
    }
  }

  /**
   * Clipboard (OSC 52 writes), links, progress (OSC 9;4), pictures (Sixel and iTerm2 inline images,
   * which is what `imgcat`, `chafa` and `timg` print) and correct widths for emoji.
   */
  private loadAddons() {
    // The cache is smaller than the addon's default: a tablet has little memory to spare, and an
    // image that falls out of it is shown as a placeholder rather than lost from the output.
    this.term.loadAddon(new ImageAddon({ storageLimit: 48 }));
    this.term.loadAddon(new ClipboardAddon(undefined, new WriteOnlyClipboard((t) => this.offerCopy(t))));
    this.term.loadAddon(new WebLinksAddon((_e, uri) => this.openLink(uri, uri)));
    this.term.loadAddon(new UnicodeGraphemesAddon());
    this.term.registerLinkProvider({ provideLinks: (y, done) => done(this.pathLinks(y)) });

    const progress = new ProgressAddon();
    this.term.loadAddon(progress);
    progress.onChange((p: Progress) => {
      const v = progressView(p);
      this.drawProgress(v);
      this.hooks.onProgress(v ? v.kind : null);
    });
  }

  /**
   * File paths on terminal line `y` (1-based), with where they are. A line the terminal wrapped is
   * read whole, so a long path split across rows still links. (Wide characters shift the columns
   * a little.)
   */
  private pathsOnLine(y: number): { range: ILink['range']; text: string; path: string; line: number | null }[] {
    const buf = this.term.buffer.active;
    const cols = this.term.cols;
    let first = y - 1;
    while (first > 0 && buf.getLine(first)?.isWrapped) first--;
    let last = y - 1;
    while (buf.getLine(last + 1)?.isWrapped) last++;
    let text = '';
    for (let r = first; r <= last; r++) text += buf.getLine(r)?.translateToString(r === last) ?? '';
    const found = [];
    for (const m of findPaths(text)) {
      const from = { row: first + Math.floor(m.start / cols), col: m.start % cols };
      const to = { row: first + Math.floor((m.end - 1) / cols), col: (m.end - 1) % cols };
      if (y - 1 < from.row || y - 1 > to.row) continue;
      const range = { start: { x: from.col + 1, y: from.row + 1 }, end: { x: to.col + 1, y: to.row + 1 } };
      found.push({ range, text: text.slice(m.start, m.end), path: m.path, line: m.line });
    }
    return found;
  }

  /** Underline and pointer on hover. xterm asks again when the pointer changes row. */
  private pathLinks(y: number): ILink[] | undefined {
    const links = this.pathsOnLine(y).map((f) => ({
      range: f.range,
      text: f.text,
      activate: () => this.openFile(f.path, f.line),
    }));
    return links.length ? links : undefined;
  }

  private lastOpened = 0;

  /** The first lines of your messages to the agent, so rows that are one can be marked. */
  setTurnSnippets(snippets: string[]) {
    this.turnSnippets = snippets;
    this.scheduleMarks();
  }

  private scheduleMarks() {
    if (this.marksTimer || this.closed) return;
    // The screen repaints in bursts while an agent streams: look once it settles.
    this.marksTimer = window.setTimeout(() => {
      this.marksTimer = 0;
      this.drawMarks();
    }, 120);
  }

  /**
   * A thin bar beside each row of the screen that is one of your messages. tmux repaints the whole
   * screen, so xterm's own markers (tied to scrolling lines) would not follow; this reads the rows
   * as drawn now.
   */
  private drawMarks() {
    const screen = this.el.querySelector('.xterm-screen');
    const buf = this.term.buffer.active;
    const rows: string[] = [];
    if (this.turnSnippets.length) {
      for (let r = 0; r < this.term.rows; r++) rows.push(buf.getLine(buf.viewportY + r)?.translateToString(true) ?? '');
    }
    const hit = screen && rows.length ? markedRows(rows, this.turnSnippets) : [];
    if (!screen || hit.length === 0) {
      this.marksEl?.replaceChildren();
      return;
    }
    if (!this.marksEl) {
      this.marksEl = el('div', 't-marks');
      this.marksEl.setAttribute('aria-hidden', 'true');
      this.el.append(this.marksEl);
    }
    const host = this.el.getBoundingClientRect();
    const sr = screen.getBoundingClientRect();
    const cell = sr.height / this.term.rows;
    this.marksEl.replaceChildren(
      ...hit.map((row) => {
        const m = el('div', 't-mark');
        m.style.cssText = `top:${sr.top - host.top + row * cell}px;height:${cell}px`;
        return m;
      }),
    );
  }

  private openFile(path: string, line: number | null) {
    this.lastOpened = performance.now();
    this.hooks.onOpenFile(path, line);
  }

  /**
   * A tap or click on a path. xterm keeps the links it found for a row while the pointer stays on
   * it, so after the text there changes (an agent redraws rows all the time) it can miss a path
   * that is there now, or open one that is gone. This reads the row fresh instead.
   */
  private watchTaps() {
    let down: { x: number; y: number } | null = null;
    this.el.addEventListener('pointerdown', (e) => (down = { x: e.clientX, y: e.clientY }), true);
    this.el.addEventListener('click', (e) => {
      const start = down;
      down = null;
      if (!start || Math.hypot(e.clientX - start.x, e.clientY - start.y) > 6 || this.term.hasSelection()) return;
      if (performance.now() - this.lastOpened < 600) return; // xterm's own link already opened it
      const screen = this.el.querySelector('.xterm-screen');
      if (!screen) return;
      const r = screen.getBoundingClientRect();
      const col = Math.floor((e.clientX - r.left) / (r.width / this.term.cols));
      const row = Math.floor((e.clientY - r.top) / (r.height / this.term.rows));
      if (col < 0 || row < 0 || col >= this.term.cols || row >= this.term.rows) return;
      const y = this.term.buffer.active.viewportY + row + 1;
      const hit = this.pathsOnLine(y).find((f) => {
        const { start: a, end: b } = f.range;
        return (y > a.y || (y === a.y && col + 1 >= a.x)) && (y < b.y || (y === b.y && col + 1 <= b.x));
      });
      if (hit) this.openFile(hit.path, hit.line);
    });
  }

  /**
   * Touch and hold opens the text as selectable text. A finger that moves, a second finger, or a
   * sheet already on screen cancels it; a mouse never starts one.
   */
  private watchLongPress() {
    let timer = 0;
    let from: { x: number; y: number } | null = null;
    let fired = 0;
    const cancel = () => {
      clearTimeout(timer);
      from = null;
    };
    this.el.addEventListener('pointerdown', (e) => {
      cancel();
      if (e.pointerType !== 'touch' || !e.isPrimary || document.querySelector('.sheet')) return;
      from = { x: e.clientX, y: e.clientY };
      timer = window.setTimeout(() => {
        from = null;
        fired = performance.now();
        this.hooks.onSelectText();
      }, 500);
    });
    this.el.addEventListener('pointermove', (e) => {
      if (from && Math.hypot(e.clientX - from.x, e.clientY - from.y) > 8) cancel();
    });
    for (const type of ['pointerup', 'pointercancel'] as const) this.el.addEventListener(type, cancel);
    // The browser's own long-press menu would land on top of the sheet.
    this.el.addEventListener('contextmenu', (e) => {
      if (performance.now() - fired < 1500) e.preventDefault();
    });
  }

  private openLink(label: string, uri: string) {
    const action = linkAction(label, uri);
    if (action === 'block') return;
    if (action === 'confirm' && !confirm(`This link says “${label}” but goes to:\n\n${uri}\n\nOpen it?`)) return;
    window.open(uri, '_blank', 'noopener,noreferrer');
  }

  /** The thin bar along the top of the terminal. */
  private drawProgress(v: ReturnType<typeof progressView>) {
    if (!v) {
      this.barEl?.remove();
      this.barEl = null;
      return;
    }
    if (!this.barEl) {
      this.barEl = el('div', 't-progress');
      this.el.append(this.barEl);
    }
    this.barEl.dataset.kind = v.kind;
    this.barEl.style.setProperty('--pct', v.pct === null ? '35%' : `${v.pct}%`);
  }

  /**
   * A short note over the terminal, replacing any earlier one.
   */
  private say(message: string, ms = 4000) {
    this.toastEl?.remove();
    clearTimeout(this.toastTimer);
    const toast = el('div', 't-toast');
    toast.append(el('span', 't-toast-msg', message));
    this.el.append(toast);
    this.toastEl = toast;
    this.toastTimer = window.setTimeout(() => toast.remove(), ms);
  }

  /**
   * Pasting or dropping files uploads them and pastes their paths into the prompt (never Enter).
   * Text pastes carry no files and keep xterm's own handling.
   */
  private watchDrops() {
    const take = (e: ClipboardEvent | DragEvent) => {
      const files = filesOf('clipboardData' in e ? e.clipboardData : e.dataTransfer);
      if (files.length === 0) return;
      e.preventDefault();
      e.stopPropagation();
      void this.attachFiles(files);
    };
    // Capture phase, so xterm's textarea never sees a file paste as an empty text paste.
    this.el.addEventListener('paste', take, true);
    this.el.addEventListener('drop', take, true);
    this.el.addEventListener('dragover', (e) => {
      if (e.dataTransfer?.types.includes('Files')) e.preventDefault();
    });
  }

  private async attachFiles(files: File[]) {
    this.say(files.length === 1 ? 'Uploading…' : `Uploading ${files.length} files…`, 30_000);
    try {
      const paths: string[] = [];
      for (const f of files) paths.push(await uploadFile(this.session, f));
      this.toastEl?.remove();
      this.term.paste(pathsToPaste(paths));
      this.term.focus();
    } catch (e) {
      if (e instanceof UnauthorizedError) return this.onUnauthorized();
      this.say(e instanceof Error && e.message ? e.message : 'Could not upload the file');
    }
  }

  /** A program asked to copy but the browser refused outside a tap: show a button that can. */
  private offerCopy(text: string) {
    this.toastEl?.remove();
    clearTimeout(this.toastTimer);
    const toast = el('div', 't-toast');
    const copy = el('button', 't-toast-btn', 'Copy');
    copy.type = 'button';
    copy.addEventListener('click', () => {
      void navigator.clipboard
        .writeText(text)
        .then(() => {
          copy.textContent = 'Copied';
        })
        .catch(() => {
          copy.textContent = 'Blocked';
        });
      this.toastTimer = window.setTimeout(() => toast.remove(), 1200);
    });
    const words = text.length;
    toast.append(el('span', 't-toast-msg', `Copy ${words} character${words === 1 ? '' : 's'}?`), copy);
    this.el.append(toast);
    this.toastEl = toast;
    this.toastTimer = window.setTimeout(() => toast.remove(), 10_000);
  }

  /**
   * Shift+Enter (and Alt+Enter) start a new line in Claude Code and Codex instead of submitting.
   * xterm sends a bare CR for all of them, so send the agent's own newline. Returns false when handled.
   */
  private newlineKey(e: KeyboardEvent): boolean {
    if (e.type !== 'keydown' || e.key !== 'Enter' || e.isComposing || e.ctrlKey || e.metaKey) return true;
    if (!e.shiftKey && !e.altKey) return true;
    const seq = newlineFor(this.hooks.agent());
    if (!seq) return true;
    e.preventDefault();
    this.sendInput(this.enc.encode(seq));
    return false;
  }

  /** Raw bytes to the PTY (used by the keybar too). */
  sendInput(bytes: Uint8Array<ArrayBuffer>) {
    if (this.ws?.readyState !== WebSocket.OPEN) return;
    if (this.scrolled) {
      // Typing while the history shows must reach the agent, not tmux's history keys.
      const { leave, forward } = inputWhileScrolled(bytes);
      if (leave) this.leaveHistory();
      if (!forward) return;
    }
    this.ws.send(bytes);
  }

  // ------------------------------------------------------------ touch scrolling

  /**
   * A finger dragged along the terminal scrolls its history. The terminal only draws the screen (tmux
   * keeps the history, and xterm has no scrollback of its own), so the drag is sent to the server as
   * lines, and tmux scrolls. A flick carries on a little after the finger lifts.
   */
  private watchTouchScroll() {
    const carry = new LineCarry();
    let id: number | null = null;
    let startY = 0;
    let lastY = 0;
    let dragging = false;
    let samples: Sample[] = [];
    const lineHeight = () => {
      const r = this.el.querySelector('.xterm-screen')?.getBoundingClientRect();
      return r && this.term.rows > 0 ? r.height / this.term.rows : 0;
    };
    const stopFling = () => {
      cancelAnimationFrame(this.flingFrame);
      this.flingFrame = 0;
    };
    const fling = (v0: number) => {
      let v = v0;
      let prev = performance.now();
      const tick = (now: number) => {
        const dt = Math.min(now - prev, 50);
        prev = now;
        const lines = carry.add(v * dt, lineHeight());
        if (lines !== 0) this.scrollBy(lines);
        v = slow(v, dt);
        // Moving down the history and already at the live screen: nothing more to do.
        if (!keepGoing(v) || (v < 0 && !this.scrolled)) {
          this.flingFrame = 0;
          return;
        }
        this.flingFrame = requestAnimationFrame(tick);
      };
      this.flingFrame = requestAnimationFrame(tick);
    };
    this.el.addEventListener('pointerdown', (e) => {
      stopFling();
      if (e.pointerType !== 'touch' || !e.isPrimary) return;
      id = e.pointerId;
      startY = lastY = e.clientY;
      dragging = false;
      carry.reset();
      samples = [{ y: e.clientY, t: performance.now() }];
    });
    this.el.addEventListener('pointermove', (e) => {
      if (e.pointerId !== id) return;
      if (!dragging) {
        if (Math.abs(e.clientY - startY) < SLOP_PX || document.querySelector('.sheet')) return;
        dragging = true;
        try {
          this.el.setPointerCapture(e.pointerId);
        } catch {
          /* the finger is already gone */
        }
      }
      const lines = carry.add(e.clientY - lastY, lineHeight());
      lastY = e.clientY;
      samples = addSample(samples, { y: e.clientY, t: performance.now() });
      if (lines !== 0) this.scrollBy(lines);
    });
    const end = (e: PointerEvent) => {
      if (e.pointerId !== id) return;
      id = null;
      if (!dragging) return;
      dragging = false;
      const v = e.type === 'pointerup' ? velocity(samples, performance.now()) : 0;
      if (flings(v)) fling(v);
    };
    this.el.addEventListener('pointerup', end);
    this.el.addEventListener('pointercancel', end);
  }

  /** Scroll by `lines` (positive: up the history). Batched, and one request at a time. */
  private scrollBy(lines: number) {
    this.scrollPending = clampLines(this.scrollPending + lines);
    if (!this.scrollTimer) this.scrollTimer = window.setTimeout(() => this.flushScroll(), SCROLL_BATCH_MS);
  }

  private flushScroll() {
    this.scrollTimer = 0;
    if (this.scrollPending === 0) return;
    // The reply to the one in flight sends the next, so they never pile up behind a slow link.
    if (this.scrollSent && performance.now() - this.scrollSent < SCROLL_LOST_MS) return;
    const lines = this.scrollPending;
    this.scrollPending = 0;
    this.scrollSent = performance.now();
    this.sendCtl({ type: 'scroll', lines });
  }

  private onScrolled(active: boolean, position: number) {
    this.scrollSent = 0;
    this.setScrolled(active ? { position } : null);
    if (this.scrollPending !== 0) this.flushScroll();
  }

  /** Show or hide the button that goes back to the live screen. */
  private setScrolled(state: { position: number } | null) {
    this.scrolled = state;
    if (!state) {
      this.liveEl?.remove();
      this.liveEl = null;
      return;
    }
    if (!this.liveEl) {
      const b = el('button', 't-live');
      b.type = 'button';
      // Keep the focus where it is: a button that took it would eat the next Space.
      b.addEventListener('pointerdown', (e) => e.preventDefault());
      b.addEventListener('click', () => this.leaveHistory());
      this.el.append(b);
      this.liveEl = b;
    }
    this.liveEl.textContent = liveLabel(state.position);
  }

  /** Back to the live screen. */
  private leaveHistory() {
    // A fling still running would scroll straight back in.
    cancelAnimationFrame(this.flingFrame);
    this.flingFrame = 0;
    this.scrollPending = 0;
    window.clearTimeout(this.scrollTimer);
    this.scrollTimer = 0;
    this.sendCtl({ type: 'scroll_end' });
    this.setScrolled(null);
  }

  focus() {
    this.term.focus();
  }

  bumpFont(delta: number) {
    this.setFont((this.term.options.fontSize ?? 14) + delta);
  }

  get font() {
    return { size: this.term.options.fontSize ?? 14, min: FONT_MIN, max: FONT_MAX };
  }

  setFont(px: number) {
    const next = Math.min(FONT_MAX, Math.max(FONT_MIN, px));
    this.term.options.fontSize = next;
    this.fit.fit();
    try {
      localStorage.setItem(FONT_KEY, String(next));
    } catch {
      /* not persisted; still applied for this page view */
    }
  }

  connect() {
    if (this.closed) return;
    this.onStatus(this.attempt ? 'reconnecting…' : 'connecting…');
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    const q = new URLSearchParams({
      session: this.session,
      rows: String(this.term.rows),
      cols: String(this.term.cols),
    });
    const ws = new WebSocket(`${proto}://${location.host}/ws?${q}`);
    ws.binaryType = 'arraybuffer';
    this.ws = ws;

    ws.onopen = () => {
      this.attempt = 0;
      this.onStatus('');
      // tmux redraws the whole screen on attach; start from a clean slate.
      this.term.reset();
      this.sendCtl({ type: 'resize', rows: this.term.rows, cols: this.term.cols });
      this.scrollPending = 0;
      this.scrollSent = 0;
      this.setScrolled(null);
      // The session may have been left showing its history by an earlier visit or another device.
      this.sendCtl({ type: 'scroll', lines: 0 });
      window.clearInterval(this.pingTimer);
      this.pingTimer = window.setInterval(() => this.sendCtl({ type: 'ping' }), PING_MS);
      window.clearInterval(this.screenTimer);
      this.screenTimer = window.setInterval(() => {
        void this.pollScreen();
        // The history may have been left some other way (a search closed, `q` typed in tmux).
        if (this.scrolled) this.sendCtl({ type: 'scroll', lines: 0 });
      }, SCREEN_POLL_MS);
      void this.pollScreen();
    };
    ws.onmessage = (e) => {
      if (typeof e.data === 'string') this.handleCtl(JSON.parse(e.data) as ServerMsg);
      else this.term.write(new Uint8Array(e.data as ArrayBuffer));
    };
    ws.onclose = () => {
      window.clearInterval(this.pingTimer);
      window.clearInterval(this.screenTimer);
      if (this.closed || this.stopped || this.ended || this.ws !== ws) return;
      void this.retry();
    };
  }

  private handleCtl(m: ServerMsg) {
    if (m.type === 'exit') {
      // The tmux session is gone; reconnecting would only start a new shell.
      this.ended = true;
      this.hooks.onScreen(null, 0);
      this.onStatus(`session ended (${m.code})`);
    } else if (m.type === 'scroll') {
      this.onScrolled(m.active, m.position);
    } else if (m.type === 'error') {
      this.onStatus(m.message);
    } else if (m.type === 'stopped') {
      this.stopped = true;
      this.onStatus('');
      void this.showSaved().then(() => this.hooks.onStopped(m.resumable));
    }
  }

  /** Show what the session last displayed, read from the server's saved copy. */
  private async showSaved() {
    const r = await fetch(`/api/sessions/${encodeURIComponent(this.session)}/output?lines=500`).catch(() => null);
    const text = r?.ok ? await r.text() : '(no saved output)';
    this.term.reset();
    this.term.write(text.replace(/\r?\n/g, '\r\n'));
  }

  private async pollScreen() {
    if (document.hidden || this.ws?.readyState !== WebSocket.OPEN) return;
    const r = await fetch(`/api/sessions/${encodeURIComponent(this.session)}/screen`).catch(() => null);
    // The reply's Date header is the server's clock: it lets a time recorded there be shown here.
    this.hooks.onScreen(r?.ok ? ((await r.json()) as Analysis) : null, skewFromDate(r?.headers.get('date') ?? null, Date.now()));
  }

  /** After a restore: leave the saved-output view and attach for real. */
  reconnect() {
    this.stopped = false;
    this.ended = false;
    this.attempt = 0;
    window.clearTimeout(this.retryTimer);
    this.term.reset();
    this.connect();
  }

  /** Send an answer for the question on screen. Returns an error message, or null on success. */
  async answer(choice: string, promptId: string | null): Promise<string | null> {
    const r = await fetch(`/api/sessions/${encodeURIComponent(this.session)}/answer`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ choice, prompt_id: promptId }),
    }).catch(() => null);
    if (!r) return 'Server unreachable';
    if (r.ok) {
      void this.pollScreen();
      return null;
    }
    return r.status === 409 ? 'The question changed, check the screen' : await r.text();
  }

  private sendCtl(m: ClientMsg) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(m));
  }

  private async retry() {
    this.attempt++;
    this.onStatus('reconnecting…');
    // A failed upgrade looks the same as a drop; ask the API whether we're still signed in.
    const me = await fetch('/api/me').catch(() => null);
    if (me?.status === 401) return this.onUnauthorized();
    const delay = Math.min(500 * 2 ** (this.attempt - 1), MAX_BACKOFF_MS);
    this.retryTimer = window.setTimeout(() => this.connect(), delay);
  }

  /** Reconnect right away (tab became visible / network came back). */
  nudge() {
    if (this.closed || this.ws?.readyState === WebSocket.OPEN) return;
    window.clearTimeout(this.retryTimer);
    this.connect();
  }

  dispose() {
    this.closed = true;
    window.clearTimeout(this.marksTimer);
    window.clearInterval(this.pingTimer);
    window.clearInterval(this.screenTimer);
    window.clearTimeout(this.retryTimer);
    window.clearTimeout(this.scrollTimer);
    cancelAnimationFrame(this.flingFrame);
    // A closed pane must not leave its session showing history, where the next keys would be lost.
    if (this.scrolled) this.sendCtl({ type: 'scroll_end' });
    this.ws?.close();
    this.cleanup();
    this.term.dispose();
  }
}
