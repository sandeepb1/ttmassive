// Searching a session's whole history. A magnifier in the corner of the terminal opens a bar:
// type, see every match in the history (not just what is on screen), and tap one to put tmux in
// copy-mode on it. tmux does the scrolling and highlighting, so the terminal shows what tmux shows.

import { el } from './dom';
import { icon } from './icons';
import {
  ago,
  counter,
  parseFound,
  parseTurns,
  parts,
  stepTo,
  stepTurn,
  turnCounter,
  upLabel,
  type Found,
  type Step,
  type Turn,
} from './search-model';

export interface SearchHooks {
  /** The session to search, now. */
  session: () => string;
  onUnauthorized: () => void;
  /** Hand the keyboard back to the terminal. */
  refocus: () => void;
  /** The first lines of your messages to the agent, whenever the list is refreshed. */
  onTurns: (snippets: string[]) => void;
}

const TYPING_DELAY_MS = 250;
/** How often your messages are looked up again, to keep the marks beside them current. */
const TURNS_POLL_MS = 15_000;

function iconButton(cls: string, label: string, glyph: Parameters<typeof icon>[0], onClick: () => void): HTMLButtonElement {
  const b = el('button', `ts-btn ${cls}`);
  b.type = 'button';
  b.setAttribute('aria-label', label);
  b.title = label;
  b.append(icon(glyph));
  b.addEventListener('pointerdown', (e) => e.preventDefault()); // keep the keyboard up
  b.addEventListener('click', onClick);
  return b;
}

export class TermSearch {
  private button: HTMLButtonElement;
  private panel = el('div', 'ts-panel');
  private input = el('input', 'ts-input');
  private count = el('span', 'ts-count');
  private list = el('div', 'ts-list');
  private query = '';
  private found: Found = { total: 0, capped: false, hits: [] };
  private current: number | null = null;
  /** Your messages to the agent, shown while the box is empty. */
  private turns: Turn[] = [];
  /** The message last jumped to, by id: the list changes as you keep talking. */
  private currentTurn: string | null = null;
  private poll = 0;
  private timer = 0;
  private run: AbortController | null = null;
  /** The session tmux was last put in copy-mode in, so closing can take it out again. */
  private jumpedIn: string | null = null;

  constructor(
    host: HTMLElement,
    private hooks: SearchHooks,
  ) {
    this.button = iconButton('ts-open', 'Search history', 'search', () => this.open());
    this.panel.hidden = true;
    this.input.type = 'search';
    this.input.placeholder = 'Search, or pick a message';
    this.input.setAttribute('aria-label', 'Search history');
    this.input.enterKeyHint = 'search';
    this.input.autocomplete = 'off';
    this.input.autocapitalize = 'off';
    this.input.spellcheck = false;
    this.input.addEventListener('input', () => this.typed());
    // The list tucks away after a jump so the match is visible; these bring it back.
    this.input.addEventListener('focus', () => this.panel.classList.remove('compact'));
    this.count.title = 'Show or hide the list';
    this.count.addEventListener('click', () => this.panel.classList.toggle('compact'));
    this.input.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        this.close();
      } else if (e.key === 'Enter') {
        e.preventDefault();
        void this.step(e.shiftKey ? 'newer' : 'older');
      }
    });

    const row = el('div', 'ts-row');
    row.append(
      this.input,
      this.count,
      iconButton('ts-older', 'Older match', 'chevron-up', () => void this.step('older')),
      iconButton('ts-newer', 'Newer match', 'chevron-down', () => void this.step('newer')),
      iconButton('ts-close', 'Close search', 'close', () => this.close()),
    );
    this.panel.append(row, this.list);
    host.append(this.button, this.panel);
    // Closing the tab after a jump must not leave the session frozen in copy-mode for next time.
    window.addEventListener('pagehide', () => this.leaveCopyMode(true));
    this.poll = window.setInterval(() => {
      if (document.visibilityState === 'visible' && host.offsetParent !== null && this.hooks.session()) void this.loadTurns(true);
    }, TURNS_POLL_MS);
  }

  private soon = 0;

  /** Look up your messages shortly: a session was just opened, and its terminal needs a moment. */
  refreshSoon() {
    window.clearTimeout(this.soon);
    this.soon = window.setTimeout(() => void this.loadTurns(true), 1500);
  }

  /** Stop looking up your messages: the pane is gone. */
  dispose() {
    window.clearInterval(this.poll);
    window.clearTimeout(this.soon);
    this.reset();
  }

  /** Step to the previous or next message of yours, without opening the bar. */
  async stepTurns(dir: Step) {
    await this.loadTurns(true);
    const i = stepTurn(this.turns, this.indexOfCurrent(), dir);
    if (i !== null) await this.jumpTurn(i);
  }

  private indexOfCurrent(): number | null {
    const i = this.turns.findIndex((t) => t.id === this.currentTurn);
    return i < 0 ? null : i;
  }

  /** Take tmux out of copy-mode if a jump put it there. */
  private leaveCopyMode(unloading = false) {
    const was = this.jumpedIn;
    this.jumpedIn = null;
    if (!was) return;
    const url = `/api/sessions/${encodeURIComponent(was)}/search/close`;
    // A page that is going away cannot be sure a fetch is sent; a beacon is.
    if (unloading && navigator.sendBeacon?.(url)) return;
    void fetch(url, { method: 'POST', keepalive: true }).catch(() => null);
  }

  get isOpen(): boolean {
    return !this.panel.hidden;
  }

  open() {
    this.panel.hidden = false;
    this.button.hidden = true;
    this.input.focus();
    this.input.select();
    // Reopening shows the last query again, but the history has moved on since.
    void this.search();
  }

  /** Close the bar and take tmux out of copy-mode if a jump put it there. */
  close() {
    clearTimeout(this.timer);
    this.run?.abort();
    this.panel.hidden = true;
    this.button.hidden = false;
    this.leaveCopyMode();
    this.hooks.refocus();
  }

  /** The pane now shows another session: forget this one's search. */
  reset() {
    this.close();
    this.input.value = '';
    this.query = '';
    this.found = { total: 0, capped: false, hits: [] };
    this.current = null;
    this.turns = [];
    this.currentTurn = null;
    this.hooks.onTurns([]);
    this.render();
  }

  /** Nothing typed: the box lists your messages instead of search results. */
  private get showingTurns(): boolean {
    return !this.input.value.trim();
  }

  private typed() {
    this.panel.classList.remove('compact');
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => void this.search(), TYPING_DELAY_MS);
  }

  private say(text: string) {
    this.count.textContent = text;
    this.list.replaceChildren();
  }

  private async search() {
    this.query = this.input.value;
    this.current = null;
    if (!this.query.trim()) return this.loadTurns();
    this.run?.abort();
    const run = (this.run = new AbortController());
    let r: Response;
    try {
      r = await fetch(`/api/sessions/${encodeURIComponent(this.hooks.session())}/search?q=${encodeURIComponent(this.query)}`, {
        signal: run.signal,
        cache: 'no-store',
      });
    } catch {
      if (!run.signal.aborted) this.say('Could not search');
      return;
    }
    if (r.status === 401) return this.hooks.onUnauthorized();
    if (r.status === 409) return this.say('Only a running session can be searched');
    if (!r.ok) return this.say(await r.text().catch(() => 'Could not search'));
    this.found = parseFound(await r.json().catch(() => null));
    this.render();
  }

  /**
   * Your messages to the agent, each placed in the terminal history where it can be found. Quiet
   * reloads (the poll, stepping) change nothing on screen except the list and the marks.
   */
  private async loadTurns(quiet = false) {
    let signal: AbortSignal | undefined;
    if (!quiet) {
      this.run?.abort(); // the previous look-up, before this one replaces it
      this.run = new AbortController();
      signal = this.run.signal;
      this.currentTurn = null;
    }
    let r: Response;
    try {
      r = await fetch(`/api/sessions/${encodeURIComponent(this.hooks.session())}/turns`, { signal, cache: 'no-store' });
    } catch {
      if (!quiet && !signal?.aborted) this.say('Could not load your messages');
      return;
    }
    if (r.status === 401) return quiet ? undefined : this.hooks.onUnauthorized();
    if (r.status === 409) return quiet ? undefined : this.say('Only a running session can be searched');
    if (!r.ok) return quiet ? undefined : this.say('Could not load your messages');
    this.turns = parseTurns(await r.json().catch(() => null));
    this.hooks.onTurns(this.turns.map((t) => t.snippet));
    if (!quiet || (this.isOpen && this.showingTurns)) this.renderTurns();
  }

  private render() {
    if (this.showingTurns) return this.renderTurns();
    const { total, capped, hits } = this.found;
    this.count.textContent = counter(this.current, total, capped);
    const rows = hits.map((h) => {
      const row = el('button', 'ts-hit');
      row.type = 'button';
      row.classList.toggle('current', h.n === this.current);
      const [before, hit, after] = parts(h);
      const text = el('span', 'ts-text');
      text.append(before, el('mark', undefined, hit), after);
      row.append(text, el('span', 'ts-up', upLabel(h.up)));
      row.addEventListener('pointerdown', (e) => e.preventDefault());
      row.addEventListener('click', () => {
        void this.jump(h.n, h.text);
        this.input.blur(); // put the on-screen keyboard away so the match is visible
      });
      return row;
    });
    this.list.replaceChildren(...rows);
    if (hits.length < total) {
      this.list.append(el('div', 'ts-more', `Showing the newest ${hits.length}. Use the arrows to reach older matches.`));
    }
    this.list.querySelector('.current')?.scrollIntoView({ block: 'nearest' });
  }

  private renderTurns() {
    this.count.textContent = turnCounter(this.turns, this.indexOfCurrent());
    const now = Date.now() / 1000;
    const rows = this.turns.map((t, i) => {
      const row = el('button', 'ts-hit ts-turn');
      row.type = 'button';
      row.classList.toggle('current', t.id === this.currentTurn);
      row.disabled = t.n === null;
      row.append(el('span', 'ts-text ts-plain', t.preview));
      const where = t.up === null ? 'not on screen' : upLabel(t.up);
      row.append(el('span', 'ts-up', [ago(t.ts, now), where].filter(Boolean).join(' · ')));
      row.addEventListener('pointerdown', (e) => e.preventDefault());
      row.addEventListener('click', () => {
        void this.jumpTurn(i);
        this.input.blur();
      });
      return row;
    });
    this.list.replaceChildren(...rows);
    if (rows.length === 0) {
      this.list.append(el('div', 'ts-more', 'Your messages to Claude or Codex in this session appear here. Type to search the history instead.'));
    }
    this.list.querySelector('.current')?.scrollIntoView({ block: 'nearest' });
  }

  /** Step to the next older or newer match (or message), even past the ones listed. */
  private async step(dir: Step) {
    if (this.showingTurns) {
      if (this.turns.length === 0) await this.loadTurns();
      const i = stepTurn(this.turns, this.indexOfCurrent(), dir);
      if (i !== null) await this.jumpTurn(i);
      return;
    }
    if (this.input.value !== this.query) await this.search(); // typed since the last search
    const n = stepTo(this.current, dir, this.found.total);
    if (n !== null) await this.jump(n, this.found.hits.find((h) => h.n === n)?.text);
  }

  /** Ask the server to put tmux in copy-mode on a match. True if it did. */
  private async send(q: string, n: number, text?: string | null): Promise<boolean> {
    const session = this.hooks.session();
    const r = await fetch(`/api/sessions/${encodeURIComponent(session)}/search/jump`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ q, n, text: text ?? undefined }),
    }).catch(() => null);
    if (r?.status === 401) {
      this.hooks.onUnauthorized();
      return false;
    }
    if (r?.status === 410) {
      void this.search(); // that match is gone: the history moved on
      return false;
    }
    if (!r?.ok) {
      this.count.textContent = r?.status === 409 ? 'Only a running session can be searched' : 'Could not jump';
      return false;
    }
    this.jumpedIn = session;
    this.panel.classList.add('compact'); // out of the way, so the match tmux scrolled to can be seen
    return true;
  }

  private async jump(n: number, text?: string) {
    if (!(await this.send(this.query, n, text))) return;
    this.current = n;
    this.render();
  }

  private async jumpTurn(i: number) {
    const t = this.turns[i];
    if (!t || t.n === null) return;
    if (!(await this.send(t.snippet, t.n, t.text))) return;
    this.currentTurn = t.id;
    this.renderTurns();
  }
}
