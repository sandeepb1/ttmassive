// The sidebar's Team Claude tab: shown when the open session's Claude or Codex goes through a
// Team Claude proxy. Health, how traffic is routed, the pooled limits, every account with its own
// limits and usage, and the requests that have gone through, with a way to move to another account.

import { getJson, sessionUrl } from './api-json';
import { el } from './dom';
import { icon } from './icons';
import { Sheet } from './sheet';
import { button } from './ui/button';
import { chip, chipClass } from './ui/chip';
import { sheetHint, sheetMsg } from './ui/sheet-parts';
import {
  STATE_LABEL,
  accountState,
  accountsFor,
  activeAccount,
  canSwitchTo,
  compact,
  dashboardReachable,
  pct,
  providerName,
  providerOf,
  recentModels,
  requestAge,
  statusKind,
  toMs,
  uptime,
  type PoolBucket,
  type TcAccount,
  type TcFeed,
  type TcReply,
  type TcStatus,
} from './teamclaude-model';
import { meter } from './usage';
import { ago } from './usage-format';

/** Re-read while the tab is open, and just to know whether the tab should exist. */
const OPEN_POLL_MS = 5000;
const AWAY_POLL_MS = 60_000;
const ALL_KEY = 'ttm.tcAllRequests';

export interface TeamHooks {
  /** Where sheets go. */
  host: () => HTMLElement;
  signedOut: () => void;
  /** Whether the open session goes through Team Claude, as last found out. */
  detected: (yes: boolean) => void;
}

const nowSecs = () => Math.floor(Date.now() / 1000);

/** A quota fraction and its reset time (ms) as a meter, or null when the account does not report it. */
function quotaMeter(label: string, fraction: number | null | undefined, resetMs: number | null | undefined): HTMLElement | null {
  const used = pct(fraction);
  if (used === null) return null;
  return meter(label, { used_pct: used, resets_at: typeof resetMs === 'number' ? Math.floor(resetMs / 1000) : null });
}

function poolMeter(label: string, b: PoolBucket | undefined): HTMLElement | null {
  return b ? quotaMeter(label, b.utilization, b.nextResetAt) : null;
}

export class TeamClaudeTab {
  readonly root = el('div', 'rb-panel tc');
  private health = el('section', 'rb-section');
  private pool = el('section', 'rb-section');
  private routing = el('section', 'rb-section');
  private accounts = el('section', 'rb-section');
  private requests = el('section', 'rb-section');
  private foot = el('div', 'tc-foot');

  private session = '';
  private active = false;
  private visible = false;
  private epoch = 0;
  private timer = 0;
  private reply: TcReply | null = null;
  private feed: TcFeed | null = null;
  private showAll = readAll();
  /** What each section last drew, so a poll that changed nothing leaves the page alone. */
  private drawn = new Map<HTMLElement, string>();

  constructor(private hooks: TeamHooks) {
    this.root.append(this.health, this.pool, this.routing, this.accounts, this.requests, this.foot);
  }

  setSession(session: string) {
    if (session === this.session) return;
    this.session = session;
    this.epoch++;
    this.reply = null;
    this.feed = null;
    this.drawn.clear();
    window.clearTimeout(this.timer);
    this.hooks.detected(false);
    this.render();
    if (this.active) void this.refresh();
  }

  /** The sidebar is on screen. */
  setActive(on: boolean) {
    if (on === this.active) return;
    this.active = on;
    window.clearTimeout(this.timer);
    if (on) void this.refresh();
  }

  /** This tab is the one showing. */
  setVisible(on: boolean) {
    if (on === this.visible) return;
    this.visible = on;
    if (this.active) void this.refresh();
  }

  // ------------------------------------------------------------------ loading

  private async refresh() {
    window.clearTimeout(this.timer);
    if (!this.session || !this.active) return;
    const epoch = this.epoch;
    const session = this.session;
    const r = await getJson<TcReply>(sessionUrl(session, 'teamclaude'), this.hooks.signedOut);
    if (epoch !== this.epoch) return;
    if (r.ok) {
      this.reply = r.data;
      this.hooks.detected(r.data.detected);
    }
    if (this.visible && this.reply?.detected && !this.reply.error) {
      const f = await getJson<TcFeed>(sessionUrl(session, `teamclaude/requests${this.showAll ? '?all=1' : ''}`), this.hooks.signedOut);
      if (epoch !== this.epoch) return;
      this.feed = f.ok ? f.data : null;
    }
    this.render();
    if (this.active) this.timer = window.setTimeout(() => void this.refresh(), this.visible ? OPEN_POLL_MS : AWAY_POLL_MS);
  }

  // ------------------------------------------------------------------ drawing

  /** Replace a section's content, unless it is what is already there. */
  private draw(box: HTMLElement, parts: (HTMLElement | null)[], signature: string) {
    if (this.drawn.get(box) === signature) return;
    this.drawn.set(box, signature);
    box.replaceChildren(...parts.filter((p): p is HTMLElement => p !== null));
    box.hidden = box.childElementCount === 0;
  }

  private render() {
    const r = this.reply;
    if (!r || !r.detected) {
      for (const box of [this.health, this.pool, this.routing, this.accounts, this.requests, this.foot]) this.draw(box, [], 'none');
      return;
    }
    if (r.error || !r.status) {
      this.draw(this.health, [el('div', 'drawer-label', 'Team Claude'), el('div', 'mem-err', r.error ?? 'Team Claude did not answer.')], `error:${r.error}`);
      for (const box of [this.pool, this.routing, this.accounts, this.requests, this.foot]) this.draw(box, [], 'none');
      return;
    }
    const provider = r.provider ?? providerOf(r.agent) ?? 'anthropic';
    const status = r.status;
    this.renderHealth(r, status, provider);
    this.renderPool(r, provider);
    this.renderRouting(status, provider);
    this.renderAccounts(status, provider);
    this.renderRequests();
    this.renderFoot(r);
  }

  private renderHealth(r: TcReply, s: TcStatus, provider: string) {
    const server = s.server ?? {};
    const card = el('div', 'rb-card tc-card');
    const top = el('div', 'rb-card-top');
    top.append(el('strong', 'rb-name', 'Team Claude'), el('span', 'rb-badge', server.version ? `v${server.version}` : 'running'));
    const facts = [
      uptime(server.uptimeSeconds) && `up ${uptime(server.uptimeSeconds)}`,
      s.sessions?.active !== undefined && `${s.sessions.active} active session${s.sessions.active === 1 ? '' : 's'}`,
      server.eventLoop?.lastLagMs !== undefined && `lag ${server.eventLoop.lastLagMs} ms`,
    ].filter(Boolean);
    card.append(top, el('div', 'mem-facts', `Routing ${providerName(provider)} through ${r.origin ?? 'the proxy'}`));
    if (facts.length > 0) card.append(el('div', 'mem-facts faint', facts.join(' · ')));
    if (server.upstream) card.append(el('div', 'mem-facts faint', `Upstream ${server.upstream}`));
    this.draw(this.health, [el('div', 'drawer-label', 'Health'), card], JSON.stringify([r.origin, server, s.sessions, provider]));
  }

  private renderPool(r: TcReply, provider: string) {
    const agg = r.quota?.aggregate;
    // The proxy pools Claude accounts; Codex accounts have their own limits, shown per account.
    if (provider !== 'anthropic' || !agg) {
      this.draw(this.pool, [], 'none');
      return;
    }
    const meters = [
      poolMeter('5 hours', agg.fiveHour),
      poolMeter('7 days', agg.weeklyShared),
      agg.weeklySonnet && agg.weeklySonnet.utilization !== agg.weeklyShared?.utilization ? poolMeter('7 days · Sonnet', agg.weeklySonnet) : null,
      agg.weeklyFable && agg.weeklyFable.utilization !== agg.weeklyShared?.utilization ? poolMeter('7 days · Fable', agg.weeklyFable) : null,
    ];
    if (meters.every((m) => m === null)) {
      this.draw(this.pool, [], 'none');
      return;
    }
    const known = agg.weeklyShared?.knownAccounts;
    const sub = known ? el('div', 'rb-hint', `Combined across ${known} Claude account${known === 1 ? '' : 's'}.`) : null;
    const box = el('div', 'u-tool-sec');
    box.append(...meters.filter((m): m is HTMLElement => m !== null));
    this.draw(this.pool, [el('div', 'drawer-label', 'Pooled limits'), box, sub], JSON.stringify(agg));
  }

  private renderRouting(s: TcStatus, provider: string) {
    const rows: [string, string][] = [];
    const current = activeAccount(s, provider);
    rows.push([`In use for ${providerName(provider)}`, current ?? 'Not reported']);
    const def = s.defaultTargets?.[provider];
    if (def && def !== current) rows.push(['Default account', def]);
    if (typeof s.switchThreshold === 'number') rows.push(['Moves on at', `${Math.round(s.switchThreshold * 100)}% used`]);
    if (s.expiryRouting?.enabled !== undefined) rows.push(['Use soonest-to-reset first', s.expiryRouting.enabled ? 'On' : 'Off']);
    if (Array.isArray(s.routes) && s.routes.length > 0) rows.push(['Routing rules', String(s.routes.length)]);
    const list = el('div', 'tc-kv');
    for (const [k, v] of rows) list.append(el('span', 'tc-k', k), el('span', 'tc-v', v));
    this.draw(this.routing, [el('div', 'drawer-label', 'Routing'), list], JSON.stringify(rows));
  }

  private renderAccounts(s: TcStatus, provider: string) {
    const own = accountsFor(s, provider);
    const others = (s.accounts ?? []).filter((a) => a.provider !== provider);
    const now = Date.now();
    const parts: HTMLElement[] = [el('div', 'drawer-label', `${providerName(provider)} accounts · ${own.length}`)];
    if (own.length === 0) parts.push(el('div', 'rb-hint', `Team Claude has no ${providerName(provider)} accounts.`));
    const list = el('div', 'tc-accounts');
    for (const a of own) list.append(this.accountCard(a, s, provider, now, true));
    parts.push(list);
    if (others.length > 0) {
      const d = el('details', 'tc-other');
      d.append(el('summary', undefined, `Other accounts · ${others.length}`));
      const inner = el('div', 'tc-accounts');
      for (const a of others) inner.append(this.accountCard(a, s, a.provider ?? '', now, false));
      d.append(inner);
      parts.push(d);
    }
    // Times in the cards move on every poll, so draw when the accounts change, not the clock.
    this.draw(this.accounts, parts, JSON.stringify([provider, s.accounts, s.currentAccounts]));
  }

  private accountCard(a: TcAccount, s: TcStatus, provider: string, nowMs: number, canAct: boolean): HTMLElement {
    const isActive = activeAccount(s, provider) === a.name && a.provider === provider;
    const state = accountState(a, isActive, nowMs);
    const card = el('div', `tc-account st-${state}`);
    const top = el('div', 'tc-account-top');
    const name = el('span', 'tc-account-name');
    name.append(el('strong', undefined, a.name));
    if (a.orgName && a.orgName !== a.name) name.append(el('span', 'faint', a.orgName));
    top.append(name, el('span', `tc-state st-${state}`, STATE_LABEL[state]));
    card.append(top);
    const meters = [
      quotaMeter('5 hours', a.quota?.unified5h, a.quota?.unified5hReset),
      quotaMeter('7 days', a.quota?.unified7d, a.quota?.unified7dReset),
    ].filter((m): m is HTMLElement => m !== null);
    if (meters.length > 0) {
      const box = el('div', 'u-tool-sec');
      box.append(...meters);
      card.append(box);
    } else {
      card.append(el('div', 'mem-facts faint', 'This account has not reported its limits yet.'));
    }
    const u = a.usage;
    if (u) {
      const bits = [
        u.totalRequests !== undefined && `${compact(u.totalRequests)} requests`,
        u.totalInputTokens !== undefined && `${compact(u.totalInputTokens)} tokens in`,
        u.totalOutputTokens !== undefined && `${compact(u.totalOutputTokens)} out`,
      ].filter(Boolean);
      if (bits.length > 0) card.append(el('div', 'mem-facts', bits.join(' · ')));
      const used = toMs(u.lastUsed);
      const models = recentModels(a);
      const recent = [used !== null && `last used ${ago(Math.floor(used / 1000), nowSecs())}`, models.length > 0 && models.join(', ')].filter(Boolean);
      if (recent.length > 0) card.append(el('div', 'mem-facts faint', recent.join(' · ')));
    }
    if (canAct && canSwitchTo(s, provider, a.name) && state !== 'disabled') {
      card.append(button({ variant: 'secondary', size: 'sm', onClick: () => this.confirmSwitch(a, s, provider) }, 'Use this account'));
    }
    return card;
  }

  private confirmSwitch(target: TcAccount, s: TcStatus, provider: string) {
    const current = accountsFor(s, provider).find((a) => a.name === activeAccount(s, provider));
    const sheet = new SwitchSheet(this.hooks.host(), this.session, this.hooks.signedOut, target, current, provider, () => void this.refresh());
    sheet.open();
  }

  private renderRequests() {
    const f = this.feed;
    const head = el('div', 'rb-uhead');
    head.append(el('div', 'drawer-label', 'Requests'));
    const toggle = el('div', 'tc-toggle');
    for (const [label, all] of [['This pane', false], ['All', true]] as const) {
      toggle.append(chip({ on: this.showAll === all, onClick: () => this.setShowAll(all) }, label));
    }
    head.append(toggle);
    const parts: HTMLElement[] = [head];
    if (!f) {
      parts.push(el('div', 'rb-hint', 'Reading the request log…'));
    } else if (!f.enabled || f.level === 'off') {
      parts.push(
        el(
          'div',
          'rb-hint',
          'Team Claude is not logging requests, or its log folder cannot be read from here. To list them, set "logDir" and "logLevel": "headers" in its config file (~/.config/teamclaude.json) and reload it.',
        ),
      );
    } else {
      if (f.level === 'body') parts.push(el('div', 'mem-note', 'Team Claude is logging full request bodies to disk. "logLevel": "headers" is enough for this list and keeps prompts out of it.'));
      if (f.requests.length === 0) {
        parts.push(el('div', 'rb-hint', this.showAll ? 'No requests have been logged yet.' : 'No requests from this conversation are in the log yet. Try All.'));
      }
      const list = el('div', 'tc-reqs');
      const now = nowSecs();
      for (const q of f.requests) {
        const row = el('div', `tc-req${q.mine ? ' mine' : ''}`);
        const code = el('span', `tc-code k-${statusKind(q)}`, q.status === null ? (q.error ? 'err' : '…') : String(q.status));
        const where = el('span', 'tc-req-path', `${q.method} ${q.path}`);
        const who = el('span', 'tc-req-who', q.retry > 0 ? `${q.account} · retry ${q.retry}` : q.account);
        const main = el('span', 'tc-req-main');
        main.append(where, who);
        row.append(code, main, el('span', 'tc-req-age', requestAge(q.at, now)));
        list.append(row);
      }
      parts.push(list);
    }
    this.draw(this.requests, parts, JSON.stringify([this.showAll, f, Math.floor(nowSecs() / 5)]));
  }

  private setShowAll(all: boolean) {
    if (all === this.showAll) return;
    this.showAll = all;
    try {
      localStorage.setItem(ALL_KEY, all ? '1' : '0');
    } catch {
      /* not remembered */
    }
    this.feed = null;
    this.render();
    void this.refresh();
  }

  private renderFoot(r: TcReply) {
    if (!dashboardReachable(r, window.location.hostname)) {
      this.draw(this.foot, [], 'none');
      return;
    }
    const a = el('a', chipClass(), 'Open Team Claude’s dashboard');
    a.href = r.dashboard ?? '#';
    a.target = '_blank';
    a.rel = 'noopener';
    a.append(icon('chevron', 'icon-svg'));
    this.draw(this.foot, [a], `dash:${r.dashboard}`);
  }
}

function readAll(): boolean {
  try {
    return localStorage.getItem(ALL_KEY) === '1';
  } catch {
    return false;
  }
}

interface SwitchReply {
  ok?: boolean;
  account?: string;
  eligible?: boolean;
  reason?: string;
}

/** "Send Claude's requests to this account instead?", with both accounts' limits to decide by. */
class SwitchSheet extends Sheet {
  constructor(
    host: HTMLElement,
    session: string,
    signedOut: () => void,
    private target: TcAccount,
    private current: TcAccount | undefined,
    private provider: string,
    private done: () => void,
  ) {
    super(host, session, 'Switch account', signedOut);
  }

  open() {
    const name = providerName(this.provider);
    const body = el('div', 'tc-switch');
    body.append(el('p', undefined, `Send ${name}’s requests to ${this.target.name}?`));
    body.append(
      sheetHint(
        'This changes Team Claude for every session that uses it, not just this one. It takes effect on the next request, and Team Claude may move on by itself when an account runs low.',
      ),
    );
    body.append(this.limits('Switch to', this.target));
    if (this.current) body.append(this.limits('Currently', this.current));
    const go = button({ size: 'sm', onClick: () => void this.switch(go) }, 'Switch');
    const cancel = button({ variant: 'secondary', size: 'sm', onClick: () => this.close() }, 'Cancel');
    const row = el('div', 'mem-actions');
    row.append(cancel, go);
    body.append(row);
    this.body.replaceChildren(body);
    this.show();
  }

  private limits(title: string, a: TcAccount): HTMLElement {
    const box = el('div', 'tc-limits');
    box.append(el('div', 'drawer-label', `${title} · ${a.name}`));
    const meters = [quotaMeter('5 hours', a.quota?.unified5h, a.quota?.unified5hReset), quotaMeter('7 days', a.quota?.unified7d, a.quota?.unified7dReset)].filter(
      (m): m is HTMLElement => m !== null,
    );
    if (meters.length === 0) box.append(sheetHint('No limits reported yet.'));
    else box.append(...meters);
    return box;
  }

  private async switch(button: HTMLButtonElement) {
    if (this.busy) return;
    this.busy = true;
    button.disabled = true;
    button.textContent = 'Switching…';
    const r = await this.call<SwitchReply>('teamclaude/switch', { account: this.target.name });
    this.busy = false;
    if (!r.ok) {
      button.disabled = false;
      button.textContent = 'Try again';
      this.body.append(sheetMsg(r.error, true));
      return;
    }
    this.done();
    if (r.data.eligible === false) {
      this.message(`Switched, but Team Claude will not send requests to ${this.target.name} yet: ${r.data.reason ?? 'it is not usable right now'}.`, true);
      return;
    }
    this.close();
  }
}
