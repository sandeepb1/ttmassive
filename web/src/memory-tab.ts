// The sidebar's Memory tab: the memsearch notes of the open session's project. It shows what is
// indexed, searches it, summarises it into decisions and open questions, and lists the daily
// notes. The tab fetches its own data, and only while it is on screen.

import { getJson, postJson, sessionUrl } from './api-json';
import { renderMarkdown } from './chat-render';
import { el } from './dom';
import { button } from './ui/button';
import { input as textInput } from './ui/input';
import { icon } from './icons';
import {
  dayLabel,
  indexedAgo,
  jobLine,
  plural,
  preview,
  scoreLabel,
  shortPath,
  type Entry,
  type Hit,
  type Job,
  type MemoryOverview,
  type RollupItem,
  type RollupView,
} from './memory-model';
import { ago } from './usage-format';

/** How often the numbers are re-read while the tab is open and nothing is running. */
const IDLE_POLL_MS = 30_000;
/** ...and while an index or a rollup is running. */
const BUSY_POLL_MS = 2000;
const MAX_DAYS = 120;
const RESULTS = 10;

interface Expanded {
  text: string;
}

export class MemoryTab {
  readonly root = el('div', 'rb-panel mem');
  private stats = el('section', 'rb-section');
  private searchBox = el('section', 'rb-section');
  private found = el('div', 'mem-found');
  private input = textInput({ type: 'search', cls: 'flex-1' });
  private rollupBox = el('section', 'rb-section');
  private timeline = el('section', 'rb-section');

  private session = '';
  private active = false;
  /** Bumped when the session changes, so a reply for the old one is dropped. */
  private epoch = 0;
  private timer = 0;
  private overview: MemoryOverview | null = null;
  private loadError = '';
  private rollup: RollupView | null = null;
  private rollupWasRunning = false;
  private hits: Hit[] | null = null;
  private query = '';
  private searching = false;
  private searchError = '';
  private daysKey = '';
  private openDays = new Set<string>();
  private dayCache = new Map<string, { bytes: number; entries: Entry[] }>();

  constructor(private signedOut: () => void) {
    this.input.type = 'search';
    this.input.placeholder = 'Search this project’s memory';
    this.input.setAttribute('aria-label', 'Search this project’s memory');
    this.input.enterKeyHint = 'search';
    this.input.autocomplete = 'off';
    const form = el('form', 'flex items-center gap-2');
    const go = button({ variant: 'secondary', size: 'icon', label: 'Search' }, icon('search'));
    go.type = 'submit';
    form.append(this.input, go);
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      void this.runSearch(this.input.value);
    });
    this.searchBox.append(el('div', 'drawer-label', 'Search'), form, this.found);
    this.root.append(this.stats, this.searchBox, this.rollupBox, this.timeline);
    this.renderAll();
  }

  /** Point the tab at another session. */
  setSession(session: string) {
    if (session === this.session) return;
    this.session = session;
    this.epoch++;
    this.overview = null;
    this.loadError = '';
    this.rollup = null;
    this.rollupWasRunning = false;
    this.hits = null;
    this.query = '';
    this.searching = false;
    this.searchError = '';
    this.daysKey = '';
    this.openDays.clear();
    this.dayCache.clear();
    this.input.value = '';
    window.clearTimeout(this.timer);
    this.renderAll();
    if (this.active) void this.refresh();
  }

  /** The tab is on screen: read the memory now and keep it fresh. */
  setActive(on: boolean) {
    if (on === this.active) return;
    this.active = on;
    window.clearTimeout(this.timer);
    if (on) void this.refresh();
  }

  // ------------------------------------------------------------------ loading

  private busy(): boolean {
    return !!(this.overview?.reindex.running || this.overview?.rollup.running || this.rollup?.job.running);
  }

  private async refresh() {
    window.clearTimeout(this.timer);
    if (!this.session || !this.active) return;
    const epoch = this.epoch;
    const r = await getJson<MemoryOverview>(sessionUrl(this.session, 'memory'), this.signedOut);
    if (epoch !== this.epoch) return;
    if (r.ok) {
      this.overview = r.data;
      this.loadError = '';
    } else {
      this.loadError = r.error;
    }
    const running = this.overview?.rollup.running ?? false;
    if (this.overview && (!this.rollup || running || this.rollupWasRunning)) {
      const v = await getJson<RollupView>(sessionUrl(this.session, 'memory/rollup'), this.signedOut);
      if (epoch !== this.epoch) return;
      if (v.ok) this.rollup = v.data;
    }
    this.rollupWasRunning = running;
    this.renderStats();
    this.renderRollup();
    this.renderTimeline();
    if (this.active) this.timer = window.setTimeout(() => void this.refresh(), this.busy() ? BUSY_POLL_MS : IDLE_POLL_MS);
  }

  private renderAll() {
    this.renderStats();
    this.renderFound();
    this.renderRollup();
    this.renderTimeline();
  }

  // ------------------------------------------------------------------ stats

  private renderStats() {
    const o = this.overview;
    if (!o) {
      this.stats.replaceChildren(
        el('div', 'drawer-label', 'Memory'),
        el('div', this.loadError ? 'rb-hint mem-err' : 'rb-hint', this.loadError || (this.session ? 'Reading the memory…' : 'Open a session to see its project’s memory.')),
      );
      return;
    }
    const card = el('div', 'rb-card mem-card');
    const top = el('div', 'rb-card-top');
    const folder = o.root.split('/').filter(Boolean).pop() ?? o.root;
    top.append(el('strong', 'rb-name', folder), el('span', 'rb-badge', o.chunks === null ? 'not indexed' : plural(o.chunks, 'chunk')));
    const facts = [plural(o.days.length, 'day'), plural(o.entries, 'entry', 'entries'), `indexed ${indexedAgo(o.last_indexed, Date.now())}`];
    const how = [o.provider, o.model].filter(Boolean).join(' · ');
    card.append(top, el('div', 'mem-facts faint', shortPath(o.root)), el('div', 'mem-facts', facts.join(' · ')));
    if (how) card.append(el('div', 'mem-facts faint', `Embeddings: ${how}`));
    if (o.index_status && o.index_status !== 'ok') card.append(el('div', 'mem-err', `The last index run: ${o.index_status}`));

    const actions = el('div', 'mem-actions');
    const reindex = button({ variant: 'secondary', size: 'sm', onClick: () => void this.reindex() }, o.reindex.running ? 'Indexing…' : 'Re-index');
    reindex.disabled = o.reindex.running || o.days.length === 0;
    actions.append(reindex);
    if (o.unindexed && !o.reindex.running) actions.append(el('span', 'mem-flag', 'Newer notes are not indexed yet'));
    const line = jobLine(o.reindex, 'Reading the notes into the index. Searching still works while this runs.', 'Indexing failed');
    this.stats.replaceChildren(el('div', 'drawer-label', 'Memory'), card, actions);
    if (line) this.stats.append(el('div', o.reindex.error ? 'mem-err' : 'mem-note', line));
    if (o.days.length === 0) {
      this.stats.append(el('div', 'rb-hint', `No notes yet in ${shortPath(o.root)}/.memsearch/memory. The memsearch plugin writes one after each turn, and they appear here.`));
    }
  }

  private async reindex() {
    if (!this.session || !this.overview) return;
    const epoch = this.epoch;
    const r = await postJson<Job>(sessionUrl(this.session, 'memory/reindex'), undefined, this.signedOut);
    if (epoch !== this.epoch) return;
    if (r.ok && this.overview) this.overview = { ...this.overview, reindex: r.data };
    else if (!r.ok) this.loadError = r.error;
    this.renderStats();
    void this.refresh();
  }

  // ------------------------------------------------------------------ search

  private async runSearch(raw: string) {
    const q = raw.trim();
    this.query = q;
    this.searchError = '';
    if (q.length < 2) {
      this.hits = null;
      this.searching = false;
      this.renderFound();
      return;
    }
    this.searching = true;
    this.renderFound();
    const epoch = this.epoch;
    const r = await getJson<{ hits: Hit[] }>(sessionUrl(this.session, `memory/search?q=${encodeURIComponent(q)}&k=${RESULTS}`), this.signedOut);
    if (epoch !== this.epoch || q !== this.query) return;
    this.searching = false;
    if (r.ok) this.hits = r.data.hits;
    else {
      this.hits = null;
      this.searchError = r.error;
    }
    this.renderFound();
  }

  private renderFound() {
    if (this.searching) {
      this.found.replaceChildren(el('div', 'mem-note', 'Searching… the first search in a while loads the model and takes a few seconds.'));
      return;
    }
    if (this.searchError) {
      this.found.replaceChildren(el('div', 'mem-err', this.searchError));
      return;
    }
    if (!this.hits) {
      this.found.replaceChildren();
      return;
    }
    if (this.hits.length === 0) {
      this.found.replaceChildren(el('div', 'rb-hint', `Nothing in memory matches “${this.query}”.`));
      return;
    }
    const today = new Date();
    const list = el('div', 'mem-hits');
    for (const h of this.hits) list.append(this.hitRow(h, today));
    this.found.replaceChildren(el('div', 'rb-results-head', plural(this.hits.length, 'result')), list);
  }

  private hitRow(h: Hit, today: Date): HTMLElement {
    const d = el('details', 'mem-hit');
    const s = el('summary');
    const main = el('span', 'mem-hit-main');
    const top = el('span', 'mem-hit-top');
    top.append(el('span', 'mem-hit-day', dayLabel(h.date, today)), el('span', 'mem-time', h.heading), el('span', 'mem-score', scoreLabel(h.score)));
    main.append(top, el('span', 'mem-preview', preview(h.text)));
    s.append(icon('chevron', 'icon-svg chev'), main);
    d.append(s);
    d.addEventListener('toggle', () => {
      if (!d.open || d.childElementCount > 1) return;
      const body = el('div', 'mem-hit-body');
      body.append(renderMarkdown(h.text.replace(/^#{1,6} .*\n?/, '')));
      const more = button({ variant: 'secondary', size: 'sm', onClick: () => void this.expand(h, body, more) }, 'Show the surrounding notes');
      body.append(more);
      d.append(body);
    });
    return d;
  }

  private async expand(h: Hit, body: HTMLElement, button: HTMLButtonElement) {
    button.disabled = true;
    button.textContent = 'Loading…';
    const epoch = this.epoch;
    const r = await getJson<Expanded>(sessionUrl(this.session, `memory/expand/${encodeURIComponent(h.chunk_hash)}`), this.signedOut);
    if (epoch !== this.epoch) return;
    if (!r.ok) {
      button.disabled = false;
      button.textContent = 'Show the surrounding notes';
      body.append(el('div', 'mem-err', r.error));
      return;
    }
    body.replaceChildren(renderMarkdown(r.data.text));
  }

  // ------------------------------------------------------------------ rollup

  private renderRollup() {
    const o = this.overview;
    if (!o) {
      this.rollupBox.replaceChildren();
      return;
    }
    const job = this.rollup?.job ?? o.rollup;
    const info = this.rollup?.rollup ?? null;
    const gen = button({ variant: 'secondary', size: 'sm', onClick: () => void this.generate() }, job.running ? 'Summarising…' : info ? 'Regenerate' : 'Generate');
    gen.disabled = job.running || o.days.length === 0;
    const head = el('div', 'rb-uhead');
    head.append(el('div', 'drawer-label', 'Decisions & open questions'), gen);
    const parts: HTMLElement[] = [head];
    const line = jobLine(job, 'Asking the model to read the notes. This takes up to a minute.', 'Summarising failed');
    if (line) parts.push(el('div', job.error ? 'mem-err' : 'mem-note', line));
    if (!info) {
      if (!job.running) {
        parts.push(
          el(
            'div',
            'rb-hint',
            `A short summary of the decisions made, the questions still open and the topics that keep coming up, written by ${o.llm_model ?? 'memsearch’s model'}. It makes one request to that model when you press Generate.`,
          ),
        );
      }
      this.rollupBox.replaceChildren(...parts);
      return;
    }
    const meta = el('div', 'mem-facts');
    meta.append(document.createTextNode(`Written by ${info.model} · ${ago(info.generated_at, Math.floor(Date.now() / 1000))}`));
    if (info.stale) meta.append(el('span', 'mem-flag', 'Notes have changed since'));
    parts.push(meta);
    const sections = [this.items('Decisions', info.decisions), this.items('Open questions', info.open_questions), this.topics(info.topics)];
    const shown = sections.filter((x): x is HTMLElement => x !== null);
    parts.push(...(shown.length > 0 ? shown : [el('div', 'rb-hint', 'The model found nothing to pull out of these notes.')]));
    this.rollupBox.replaceChildren(...parts);
  }

  private items(title: string, items: RollupItem[]): HTMLElement | null {
    if (items.length === 0) return null;
    const box = el('div', 'mem-sum');
    const list = el('ul', 'mem-list');
    for (const i of items) {
      const li = el('li');
      li.append(document.createTextNode(i.text));
      if (i.date) li.append(el('span', 'mem-when', ` ${dayLabel(i.date, new Date())}`));
      list.append(li);
    }
    box.append(el('div', 'mem-sub', title), list);
    return box;
  }

  private topics(topics: { name: string; summary: string }[]): HTMLElement | null {
    if (topics.length === 0) return null;
    const box = el('div', 'mem-sum');
    const list = el('ul', 'mem-list');
    for (const t of topics) {
      const li = el('li');
      li.append(el('strong', undefined, t.name));
      if (t.summary) li.append(document.createTextNode(` — ${t.summary}`));
      list.append(li);
    }
    box.append(el('div', 'mem-sub', 'Topics'), list);
    return box;
  }

  private async generate() {
    if (!this.session) return;
    const epoch = this.epoch;
    const r = await postJson<Job>(sessionUrl(this.session, 'memory/rollup'), undefined, this.signedOut);
    if (epoch !== this.epoch) return;
    if (r.ok) this.rollup = { rollup: this.rollup?.rollup ?? null, job: r.data };
    else this.rollup = { rollup: this.rollup?.rollup ?? null, job: { running: false, started: 0, finished: 0, error: r.error } };
    this.rollupWasRunning = true;
    this.renderRollup();
    void this.refresh();
  }

  // ------------------------------------------------------------------ timeline

  private renderTimeline() {
    const o = this.overview;
    if (!o || o.days.length === 0) {
      this.daysKey = '';
      this.timeline.replaceChildren();
      return;
    }
    // Rebuilding collapses what the reader has open, so only do it when the notes changed.
    const key = JSON.stringify(o.days);
    if (key === this.daysKey) return;
    this.daysKey = key;
    const today = new Date();
    const list = el('div', 'mem-days');
    for (const day of o.days.slice(0, MAX_DAYS)) {
      const d = el('details', 'mem-day');
      d.open = this.openDays.has(day.date);
      const s = el('summary');
      s.append(icon('chevron', 'icon-svg chev'), el('span', 'mem-day-name', dayLabel(day.date, today)), el('span', 'mem-day-count', plural(day.entries, 'entry', 'entries')));
      const body = el('div', 'mem-day-body');
      d.append(s, body);
      d.addEventListener('toggle', () => {
        if (d.open) {
          this.openDays.add(day.date);
          this.fillDay(body, day.date, day.bytes);
        } else {
          this.openDays.delete(day.date);
        }
      });
      if (d.open) this.fillDay(body, day.date, day.bytes);
      list.append(d);
    }
    const parts: HTMLElement[] = [el('div', 'drawer-label', 'Notes by day'), list];
    if (o.days.length > MAX_DAYS) parts.push(el('div', 'rb-hint', `The ${o.days.length - MAX_DAYS} oldest days are not listed.`));
    this.timeline.replaceChildren(...parts);
  }

  private fillDay(box: HTMLElement, date: string, bytes: number) {
    const cached = this.dayCache.get(date);
    if (cached && cached.bytes === bytes) {
      box.replaceChildren(...this.entries(cached.entries));
      return;
    }
    box.replaceChildren(el('div', 'mem-note', 'Loading…'));
    const epoch = this.epoch;
    void getJson<{ entries: Entry[] }>(sessionUrl(this.session, `memory/day/${date}`), this.signedOut).then((r) => {
      if (epoch !== this.epoch) return;
      if (!r.ok) {
        box.replaceChildren(el('div', 'mem-err', r.error));
        return;
      }
      this.dayCache.set(date, { bytes, entries: r.data.entries });
      box.replaceChildren(...this.entries(r.data.entries));
    });
  }

  private entries(entries: Entry[]): HTMLElement[] {
    if (entries.length === 0) return [el('div', 'rb-hint', 'This note has no entries.')];
    return entries.map((e) => {
      const box = el('div', 'mem-entry');
      const head = el('div', 'mem-entry-head');
      head.append(el('span', 'mem-time', e.time));
      if (e.group) head.append(el('span', 'mem-group', e.group));
      const list = el('ul', 'mem-list');
      for (const b of e.bullets) list.append(el('li', undefined, b));
      box.append(head, list);
      return box;
    });
  }
}
