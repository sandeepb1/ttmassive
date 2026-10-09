import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { KeyBar } from './keybar';
import type { Analysis, ClientMsg, ServerMsg } from './protocol';

const PING_MS = 20_000;
const SCREEN_POLL_MS = 2000;

export interface SessionHooks {
  /** The session exists only as saved output; offer a restore. */
  onStopped: (resumable: boolean) => void;
  /** Latest screen analysis, or null when unavailable. */
  onScreen: (a: Analysis | null) => void;
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
    theme: { background: '#0b0d10' },
  });
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
  ) {
    this.keybar = new KeyBar(
      keybarEl,
      (b) => this.sendInput(b),
      () => this.term.modes.applicationCursorKeysMode,
      () => this.term.focus(),
    );
    this.term.loadAddon(this.fit);
    this.term.open(el);
    this.term.onData((d) => this.sendInput(this.enc.encode(this.keybar.transform(d))));
    this.term.onBinary((d) => this.sendInput(Uint8Array.from(d, (c) => c.charCodeAt(0))));
    this.term.onResize(({ rows, cols }) => this.sendCtl({ type: 'resize', rows, cols }));

    // Orientation change, soft keyboard and window resizes all land here.
    const refit = () => requestAnimationFrame(() => this.fit.fit());
    const ro = new ResizeObserver(refit);
    ro.observe(el);
    window.addEventListener('orientationchange', refit);
    window.visualViewport?.addEventListener('resize', refit);
    this.cleanup = () => {
      ro.disconnect();
      window.removeEventListener('orientationchange', refit);
      window.visualViewport?.removeEventListener('resize', refit);
    };
    this.fit.fit();
  }

  /** Raw bytes to the PTY (used by the keybar too). */
  sendInput(bytes: Uint8Array<ArrayBuffer>) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(bytes);
  }

  focus() {
    this.term.focus();
  }

  bumpFont(delta: number) {
    const next = Math.min(FONT_MAX, Math.max(FONT_MIN, (this.term.options.fontSize ?? 14) + delta));
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
      this.hooks.onScreen(null);
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
    this.hooks.onScreen(r?.ok ? ((await r.json()) as Analysis) : null);
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
    window.clearInterval(this.pingTimer);
    window.clearInterval(this.screenTimer);
    window.clearTimeout(this.retryTimer);
    this.ws?.close();
    this.cleanup();
    this.term.dispose();
  }
}
