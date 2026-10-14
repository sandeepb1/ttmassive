import { Terminal, type ILink } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { ClipboardAddon, type IClipboardProvider } from '@xterm/addon-clipboard';
import { ProgressAddon } from '@xterm/addon-progress';
import { UnicodeGraphemesAddon } from '@xterm/addon-unicode-graphemes';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { el } from './dom';
import { KeyBar } from './keybar';
import type { Analysis, ClientMsg, ServerMsg } from './protocol';
import { MONO_FALLBACK, MONO_FONT, onThemeChange, xtermTheme } from './theme';
import { linkAction, progressView, type Progress } from './term-addons';
import { skewFromDate } from './activity-chip';
import { findPaths } from './paths';
import { markedRows } from './turn-marks';

const PING_MS = 20_000;
const SCREEN_POLL_MS = 2000;

export interface SessionHooks {
  /** The session exists only as saved output; offer a restore. */
  onStopped: (resumable: boolean) => void;
  /** Latest screen analysis, or null when unavailable. */
  onScreen: (a: Analysis | null, skewMs: number) => void;
  /** The program reported progress (OSC 9;4). `kind` is null when it cleared it. */
  onProgress: (kind: 'normal' | 'error' | 'indeterminate' | 'paused' | null) => void;
  /** A file path in the output was tapped. */
  onOpenFile: (path: string, line: number | null) => void;
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
    );
    this.term.loadAddon(this.fit);
    this.term.open(el);
    this.setGpu(gpu);
    this.loadAddons();
    this.watchTaps();
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
    this.cleanup = () => {
      offTheme();
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

  /** Clipboard (OSC 52 writes), links, progress (OSC 9;4) and correct widths for emoji. */
  private loadAddons() {
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

  /** Raw bytes to the PTY (used by the keybar too). */
  sendInput(bytes: Uint8Array<ArrayBuffer>) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(bytes);
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
      window.clearInterval(this.pingTimer);
      this.pingTimer = window.setInterval(() => this.sendCtl({ type: 'ping' }), PING_MS);
      window.clearInterval(this.screenTimer);
      this.screenTimer = window.setInterval(() => void this.pollScreen(), SCREEN_POLL_MS);
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
    this.ws?.close();
    this.cleanup();
    this.term.dispose();
  }
}
