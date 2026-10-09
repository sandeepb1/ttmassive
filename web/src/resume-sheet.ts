// The resume sheet: the agent's own `/resume` list, with a search box, paging and one tap to
// resume a conversation. It stands in for sending "/resume" as a chat message.

import { el } from './dom';
import { Sheet, row } from './sheet';

interface ResumeRow {
  title: string;
  meta: string;
  open_elsewhere: boolean;
}

interface ResumeList {
  rows: ResumeRow[];
  more_above: boolean;
  more_below: boolean;
  position: number | null;
  total: number | null;
  query: string;
  can_scope: boolean;
}

export interface ResumeHooks {
  onUnauthorized: () => void;
  /** A conversation was picked and the agent is loading it. */
  onResumed: (message: string) => void;
}

export class ResumeSheet extends Sheet {
  private list: ResumeList | null = null;
  /** The agent's list is open on the host and still needs closing. */
  private listOpen = false;
  private note = '';

  constructor(
    host: HTMLElement,
    session: string,
    private hooks: ResumeHooks,
  ) {
    super(host, session, 'Resume a conversation', hooks.onUnauthorized);
  }

  async open() {
    if (!this.gone) return;
    this.show();
    this.message('Opening the list…');
    const r = await this.call<ResumeList>('resume/open');
    if (!r.ok) return this.message(r.error, true);
    this.listOpen = true;
    this.list = r.data;
    this.render();
  }

  protected override onClose() {
    if (this.listOpen) {
      this.listOpen = false;
      void this.call('model/close');
    }
  }

  private render() {
    const l = this.list;
    if (!l) return;
    const parts: HTMLElement[] = [];
    if (this.note) parts.push(el('div', 'sheet-msg err', this.note));

    // Search
    const form = el('form', 'resume-search');
    const input = el('input', 'resume-input');
    input.type = 'search';
    input.placeholder = 'Search conversations';
    input.value = l.query;
    input.enterKeyHint = 'search';
    input.setAttribute('aria-label', 'Search conversations');
    const go = el('button', 'resume-go', 'Search');
    go.type = 'submit';
    form.append(input, go);
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      void this.run(() => this.call<ResumeList>('resume/search', { query: input.value.trim(), previous: l.query.length }));
    });
    parts.push(form);

    if (l.can_scope) {
      const all = el('button', 'confirm-no', 'Switch between this project and all projects');
      all.type = 'button';
      all.addEventListener('click', () => void this.run(() => this.call<ResumeList>('resume/scope')));
      parts.push(all);
    }

    // The sessions
    if (l.rows.length === 0) {
      parts.push(el('div', 'sheet-hint', l.query ? 'Nothing matches that search.' : 'No earlier conversations here.'));
    }
    const rows = el('div', 'sheet-list');
    l.rows.forEach((r, i) => {
      const b = row(r.title, { hint: r.meta, mark: r.open_elsewhere ? 'open elsewhere' : undefined });
      b.addEventListener('click', () => void this.pick(r, i));
      rows.append(b);
    });
    parts.push(rows);

    // Paging
    const where = l.position && l.total ? `${l.position} of ${l.total}` : '';
    const pager = el('div', 'resume-pager');
    const newer = el('button', 'confirm-no', 'Newer');
    const older = el('button', 'confirm-no', 'Older');
    for (const b of [newer, older]) b.type = 'button';
    newer.disabled = !l.more_above && (l.position ?? 1) <= 1;
    older.disabled = !l.more_below;
    newer.addEventListener('click', () => void this.run(() => this.call<ResumeList>('resume/page', { down: false })));
    older.addEventListener('click', () => void this.run(() => this.call<ResumeList>('resume/page', { down: true })));
    pager.append(newer, el('span', 'resume-where', where), older);
    parts.push(pager);
    this.body.replaceChildren(...parts);
  }

  /** Run a call that returns a fresh list, and show it. */
  private async run(call: () => Promise<{ ok: true; data: ResumeList } | { ok: false; error: string }>) {
    if (this.busy) return;
    this.busy = true;
    this.note = '';
    const r = await call();
    this.busy = false;
    if (r.ok) this.list = r.data;
    else this.note = r.error;
    this.render();
  }

  private async pick(r: ResumeRow, index: number) {
    if (this.busy) return;
    this.busy = true;
    this.message(`Resuming ${r.title}…`);
    const res = await this.call<{ message: string }>('resume/choose', { title: r.title, meta: r.meta, index });
    this.busy = false;
    this.listOpen = false; // the server closes the list whatever happened
    if (!res.ok) return this.message(res.error, true);
    this.close();
    this.hooks.onResumed(res.data.message);
  }
}
