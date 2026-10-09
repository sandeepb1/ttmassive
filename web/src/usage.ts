// Plan-limit usage: a one-line strip that is always on screen, and a panel that opens when
// you tap it. The numbers come from `/api/usage`; the server decides how fresh they are.

import { el } from './dom';
import {
  SOURCE_LABEL,
  ago,
  countdown,
  isStale,
  level,
  stripSegments,
  tokenLine,
  type LimitWindow,
  type ToolUsage,
  type UsageSnapshot,
} from './usage-format';

const POLL_MS = 60_000;
/** While the panel is open, redraw this often so the countdowns keep moving. */
const TICK_MS = 15_000;

const now = () => Math.floor(Date.now() / 1000);

function meter(label: string, w: LimitWindow): HTMLElement {
  const lvl = level(w.used_pct);
  const row = el('div', 'u-win');
  const top = el('div', 'u-win-top');
  top.append(el('span', undefined, label), el('span', `u-pct lvl-${lvl}`, `${Math.round(w.used_pct)}%`));
  const track = el('div', 'meter');
  const fill = el('div', `meter-fill lvl-${lvl}`);
  fill.style.width = `${Math.min(100, Math.max(0, w.used_pct))}%`;
  track.append(fill);
  const left = countdown(w.resets_at, now());
  row.append(top, track, el('div', 'u-win-sub', left === '' ? 'reset time unknown' : left === 'now' ? 'resets now' : `resets in ${left}`));
  return row;
}

export class UsageView {
  private panel = el('aside', 'drawer usage-panel');
  private body = el('div', 'usage-body');
  private snap: UsageSnapshot | null = null;
  private unreachable = false;
  private pollTimer = 0;
  private tickTimer = 0;
  private inflight = false;

  constructor(
    private strip: HTMLElement,
    host: HTMLElement,
    private onUnauthorized: () => void,
  ) {
    this.panel.hidden = true;
    this.panel.setAttribute('aria-label', 'Plan usage');

    const head = el('div', 'drawer-head');
    head.append(el('strong', undefined, 'Plan usage'));
    const close = el('button', 'drawer-x', '×');
    close.type = 'button';
    close.setAttribute('aria-label', 'Close');
    close.addEventListener('click', () => this.hide());
    head.append(close);

    const refresh = el('button', 'drawer-logout', 'Refresh now');
    refresh.type = 'button';
    refresh.addEventListener('click', () => void this.load(true));

    this.panel.append(head, this.body, refresh);
    host.append(this.panel);
    this.render();
  }

  /** Begin polling. Safe to call again after `stop`. */
  start() {
    this.stop();
    void this.load(false);
    this.pollTimer = window.setInterval(() => void this.load(false), POLL_MS);
  }

  stop() {
    window.clearInterval(this.pollTimer);
    this.pollTimer = 0;
    this.hide();
  }

  /** Back on screen after the tab slept: timers were throttled, so catch up now. */
  nudge() {
    if (this.pollTimer) void this.load(false);
  }

  toggle() {
    if (this.panel.hidden) this.show();
    else this.hide();
  }

  show() {
    this.panel.hidden = false;
    this.render();
    void this.load(true);
    window.clearInterval(this.tickTimer);
    this.tickTimer = window.setInterval(() => this.render(), TICK_MS);
  }

  hide() {
    this.panel.hidden = true;
    window.clearInterval(this.tickTimer);
  }

  /** What a session has used so far, for the session list. */
  tokensLabel(id: string): string | undefined {
    const t = this.snap?.sessions[id];
    return t ? tokenLine(t) : undefined;
  }

  private async load(refresh: boolean) {
    if (document.hidden && !refresh) return;
    if (this.inflight && !refresh) return;
    this.inflight = true;
    try {
      const r = await fetch(`/api/usage${refresh ? '?refresh=1' : ''}`);
      if (r.status === 401) return this.onUnauthorized();
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      this.snap = (await r.json()) as UsageSnapshot;
      this.unreachable = false;
    } catch {
      this.unreachable = true;
    } finally {
      this.inflight = false;
    }
    this.render();
  }

  private render() {
    this.renderStrip();
    if (!this.panel.hidden) this.renderPanel();
  }

  private renderStrip() {
    const parts: HTMLElement[] = [];
    if (!this.snap) {
      parts.push(el('span', 'u-na', this.unreachable ? 'usage unavailable' : 'usage…'));
    } else {
      const t = now();
      let summary = '';
      for (const [short, name, u] of [
        ['CC', 'Claude Code', this.snap.claude],
        ['CX', 'Codex', this.snap.codex],
      ] as const) {
        if (parts.length) parts.push(el('span', 'u-sep', '│'));
        const tool = el('span', isStale(u, t) ? 'u-tool stale' : 'u-tool');
        tool.append(el('span', 'u-name', short));
        const { items, unavailable } = stripSegments(u);
        if (unavailable) tool.append(el('span', 'u-na', u.error ? '⚠' : '–'));
        for (const it of items) tool.append(el('span', `u-item lvl-${it.level}`, it.text));
        parts.push(tool);
        summary += `${name}: ${unavailable ? (u.error ?? 'no data') : items.map((i) => i.text).join(', ')}. `;
      }
      this.strip.title = summary.trim();
    }
    this.strip.replaceChildren(...parts);
  }

  private renderPanel() {
    const snap = this.snap;
    const sections: HTMLElement[] = [];
    if (this.unreachable) sections.push(el('div', 'u-err', 'Could not reach the server. Showing the last numbers.'));
    if (!snap) {
      sections.push(el('div', 'drawer-empty', 'Loading…'));
    } else {
      sections.push(this.toolSection('Claude Code', snap.claude), this.toolSection('Codex', snap.codex));
      sections.push(this.sessionSection(snap));
    }
    this.body.replaceChildren(...sections);
  }

  private toolSection(name: string, u: ToolUsage): HTMLElement {
    const sec = el('section', 'u-tool-sec');
    const head = el('div', 'u-tool-head');
    head.append(el('strong', undefined, u.plan ? `${name} · ${u.plan}` : name));
    head.append(
      el('span', isStale(u, now()) ? 'u-age stale' : 'u-age', `${ago(u.fetched_at, now())} · ${SOURCE_LABEL[u.source]}`),
    );
    sec.append(head);
    if (u.five_hour) sec.append(meter('5-hour', u.five_hour));
    if (u.seven_day) sec.append(meter('Weekly', u.seven_day));
    for (const m of u.per_model) sec.append(meter(`${m.name.replaceAll('_', ' ')} weekly`, m));
    if (!u.five_hour && !u.seven_day) sec.append(el('div', 'drawer-empty', 'No numbers yet'));
    if (u.error) sec.append(el('div', 'u-err', `⚠ ${u.error}`));
    return sec;
  }

  private sessionSection(snap: UsageSnapshot): HTMLElement {
    const sec = el('section', 'u-tool-sec');
    sec.append(el('div', 'drawer-label', 'Tokens per session'));
    const rows = Object.entries(snap.sessions).sort((a, b) => b[1].output - a[1].output);
    if (rows.length === 0) {
      sec.append(el('div', 'drawer-empty', 'No Claude or Codex sessions with a conversation yet'));
    }
    for (const [id, t] of rows) {
      const row = el('div', 'u-sess');
      row.append(el('span', 'u-sess-name', id), el('span', 'srow-meta', `${t.tool} · ${tokenLine(t)}`));
      sec.append(row);
    }
    return sec;
  }
}
