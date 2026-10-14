// The command bar (Cmd/Ctrl+K): a text box over a ranked list of things to do. A command either runs
// or opens a second step, like picking a folder and then what to start in it.

import { el } from './dom';
import { pushOverlay } from './overlays';
import { rank, type Item, type Step } from './command-model';

const MAX_SHOWN = 50;
/** How long to wait after a keystroke before asking the server for folders. */
const SERVER_DEBOUNCE_MS = 120;

export interface BarHooks {
  /** Ids of recently used commands, newest first. */
  recent: () => string[];
  /** A command was chosen. */
  used: (id: string) => void;
  /** The bar went away: give the keyboard back to the terminal. */
  closed: () => void;
}

export class CommandBar {
  private backdrop = el('div', 'cmd-backdrop');
  private panel = el('div', 'cmd');
  private input = el('input', 'cmd-input');
  private list = el('ul', 'cmd-list');
  private stack: Step[] = [];
  private shown: Item[] = [];
  private active = 0;
  private seq = 0;
  private timer = 0;
  /** The refresh that is running, and one waiting out the server debounce. */
  private pending: Promise<void> = Promise.resolve();
  private flush: (() => Promise<void>) | null = null;
  private open_ = false;
  private releaseOverlay: (() => void) | null = null; // Back closes the bar; see overlay-stack.ts

  constructor(
    private host: HTMLElement,
    private hooks: BarHooks,
  ) {
    this.panel.setAttribute('role', 'dialog');
    this.panel.setAttribute('aria-label', 'Commands');
    this.input.type = 'text';
    this.input.autocomplete = 'off';
    this.input.spellcheck = false;
    this.input.autocapitalize = 'off';
    this.input.setAttribute('role', 'combobox');
    this.input.setAttribute('aria-controls', 'cmd-list');
    this.input.setAttribute('aria-expanded', 'true');
    this.list.id = 'cmd-list';
    this.list.setAttribute('role', 'listbox');
    this.panel.append(this.input, this.list);
    this.backdrop.addEventListener('click', () => this.close());
    // Keep the box focused (and the soft keyboard up) when a row is pressed.
    this.list.addEventListener('mousedown', (e) => e.preventDefault());
    this.list.addEventListener('click', (e) => {
      const row = (e.target as Element).closest<HTMLElement>('[data-i]');
      if (row) this.choose(this.shown[Number(row.dataset.i)]);
    });
    this.input.addEventListener('input', () => this.refresh());
    this.input.addEventListener('keydown', (e) => this.key(e));
  }

  get isOpen() {
    return this.open_;
  }

  open(root: Step) {
    if (this.open_) return;
    this.open_ = true;
    this.stack = [root];
    this.host.append(this.backdrop, this.panel);
    this.releaseOverlay = pushOverlay(() => this.close());
    this.enter();
  }

  close() {
    if (!this.open_) return;
    this.open_ = false;
    this.releaseOverlay?.();
    this.releaseOverlay = null;
    window.clearTimeout(this.timer);
    this.seq++;
    this.backdrop.remove();
    this.panel.remove();
    this.hooks.closed();
  }

  private get step(): Step {
    return this.stack[this.stack.length - 1]!;
  }

  /** Show the top step with an empty box. */
  private enter() {
    this.input.value = '';
    this.input.placeholder = this.step.placeholder;
    this.input.focus();
    this.refresh(true);
  }

  private back() {
    if (this.stack.length > 1) {
      this.stack.pop();
      this.enter();
    } else this.close();
  }

  private key(e: KeyboardEvent) {
    switch (e.key) {
      case 'ArrowDown':
      case 'ArrowUp': {
        e.preventDefault();
        if (this.shown.length === 0) return;
        this.active = (this.active + (e.key === 'ArrowDown' ? 1 : -1) + this.shown.length) % this.shown.length;
        this.paintActive();
        return;
      }
      case 'Enter':
        e.preventDefault();
        if (!e.isComposing) void this.settled().then(() => this.choose(this.shown[this.active]));
        return;
      case 'Tab':
        if (this.shown[this.active]?.complete === undefined) return;
        e.preventDefault();
        void this.settled().then(() => {
          const done = this.shown[this.active]?.complete;
          if (done === undefined) return;
          this.input.value = done;
          this.refresh(true);
        });
        return;
      case 'Escape':
        e.preventDefault();
        this.back();
        return;
      case 'Backspace':
        if (this.input.value === '' && this.stack.length > 1) {
          e.preventDefault();
          this.back();
        }
        return;
    }
  }

  private choose(item: Item | undefined) {
    if (!item) return;
    if (item.step) {
      this.stack.push(item.step());
      this.enter();
      return;
    }
    this.hooks.used(item.id);
    this.close();
    item.run?.();
  }

  /** Wait until the list shows what is typed, skipping the debounce for a server step. */
  private async settled() {
    if (this.flush) {
      window.clearTimeout(this.timer);
      const run = this.flush;
      this.flush = null;
      this.pending = run();
    }
    await this.pending;
  }

  private refresh(now = false) {
    window.clearTimeout(this.timer);
    this.flush = null;
    const step = this.step;
    const mine = ++this.seq;
    const go = async () => {
      const query = this.input.value;
      let items: Item[];
      try {
        items = await step.items(query);
      } catch {
        items = [];
      }
      if (mine !== this.seq || !this.open_) return; // a later keystroke or step took over
      this.shown = (step.filtered ? items : rank(items, query, this.hooks.recent())).slice(0, MAX_SHOWN);
      this.active = 0;
      this.paint();
    };
    if (now || !step.filtered) {
      this.pending = go();
      return;
    }
    this.flush = go;
    this.timer = window.setTimeout(() => {
      this.flush = null;
      this.pending = go();
    }, SERVER_DEBOUNCE_MS);
  }

  private paint() {
    if (this.shown.length === 0) {
      this.list.replaceChildren(el('li', 'cmd-empty', 'No matches'));
      this.input.removeAttribute('aria-activedescendant');
      return;
    }
    this.list.replaceChildren(
      ...this.shown.map((it, i) => {
        const li = el('li', 'cmd-row');
        li.id = `cmd-opt-${i}`;
        li.dataset.i = String(i);
        li.setAttribute('role', 'option');
        const text = el('span', 'cmd-text');
        text.append(el('span', 'cmd-label', it.label));
        if (it.hint) text.append(el('span', 'cmd-hint', it.hint));
        li.append(text);
        if (it.group) li.append(el('span', 'cmd-group', it.group));
        if (it.step) li.append(el('span', 'cmd-more', '›'));
        return li;
      }),
    );
    this.paintActive();
  }

  private paintActive() {
    this.list.querySelectorAll('.cmd-row').forEach((r, i) => {
      const on = i === this.active;
      r.classList.toggle('active', on);
      r.setAttribute('aria-selected', String(on));
      if (on) {
        this.input.setAttribute('aria-activedescendant', r.id);
        (r as HTMLElement).scrollIntoView({ block: 'nearest' });
      }
    });
  }
}
