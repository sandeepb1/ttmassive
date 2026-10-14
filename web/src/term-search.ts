// Searching a session's whole history. A magnifier in the corner of the terminal opens a bar:
// type, see every match in the history (not just what is on screen), and tap one to put tmux in
// copy-mode on it. tmux does the scrolling and highlighting, so the terminal shows what tmux shows.

import { el } from './dom';
import { icon } from './icons';
import { counter, parseFound, parts, stepTo, upLabel, type Found, type Step } from './search-model';

export interface SearchHooks {
  /** The session to search, now. */
  session: () => string;
  onUnauthorized: () => void;
  /** Hand the keyboard back to the terminal. */
  refocus: () => void;
}

const TYPING_DELAY_MS = 250;

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
    this.input.placeholder = 'Search this session’s history';
    this.input.setAttribute('aria-label', 'Search history');
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
    if (this.input.value.trim()) void this.search();
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
    this.render();
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
    if (!this.query.trim()) {
      this.found = { total: 0, capped: false, hits: [] };
      return this.render();
    }
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

  private render() {
    const { total, capped, hits } = this.found;
    this.count.textContent = this.query.trim() ? counter(this.current, total, capped) : '';
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

  /** Step to the next older or newer match, even past the ones listed. */
  private async step(dir: Step) {
    if (this.input.value !== this.query) await this.search(); // typed since the last search
    const n = stepTo(this.current, dir, this.found.total);
    if (n !== null) await this.jump(n, this.found.hits.find((h) => h.n === n)?.text);
  }

  private async jump(n: number, text?: string) {
    const session = this.hooks.session();
    const r = await fetch(`/api/sessions/${encodeURIComponent(session)}/search/jump`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ q: this.query, n, text }),
    }).catch(() => null);
    if (r?.status === 401) return this.hooks.onUnauthorized();
    if (r?.status === 410) return void this.search(); // that match is gone: the history moved on
    if (!r?.ok) {
      this.count.textContent = r?.status === 409 ? 'Only a running session can be searched' : 'Could not jump';
      return;
    }
    this.jumpedIn = session;
    this.current = n;
    this.panel.classList.add('compact'); // out of the way, so the match tmux scrolled to can be seen
    this.render();
  }
}
