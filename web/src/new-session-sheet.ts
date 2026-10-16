// The new-session sheet: pick a folder (type it, take a recent or pinned project, or browse), pick
// what to start in it (Claude, Codex, a shell or another preset) and go. It is the one way in for
// the sidebar's `+`, a project's own `+` and the command bar's "New session…".

import { DRIVER_LABEL, driverFor, isWeb, orAccounts, shortModel, priceLine, type Account, type Driver } from './accounts-model';
import { getJson } from './api-json';
import { DirectChatSheet } from './direct-chat-sheet';
import { el } from './dom';
import {
  browseDirs, cachedNewSessionData, canSkip, launchGroup, launchSession, loadNewSessionData, mainAgents,
  type BrowseResult, type GroupInfo, type NewSessionData, type PresetInfo, type RecentDir,
} from './dirs';
import { icon } from './icons';
import { Sheet } from './sheet';
import { button } from './ui/button';
import { chip } from './ui/chip';
import { input as textInput, select } from './ui/input';
import { sheetLabel, sheetMsg } from './ui/sheet-parts';

const AGENT_KEY = 'ttm.new.agent';
const OR_KEY = 'ttm.new.openrouter';
const SHELL = '';
/** The agent choice that runs Claude Code or Codex on an OpenRouter model. It cannot clash with a preset name. */
const OR_CHOICE = '@openrouter';
// A plain chat with a model on one of your accounts: opens its own sheet and starts nothing.
const CHAT_CHOICE = '@chat';

interface Folder {
  path: string;
  short: string;
}

export interface NewSessionHooks {
  /** A session was started: show it. */
  opened: (id: string) => void;
  /** Project folders kept in the list, to offer first. */
  pinned: () => string[];
}

function remembered(): string | null {
  try {
    return localStorage.getItem(AGENT_KEY);
  } catch {
    return null;
  }
}

function remember(choice: string) {
  try {
    localStorage.setItem(AGENT_KEY, choice);
  } catch {
    /* the choice just is not remembered */
  }
}

function rememberedOr(): { account?: string; model?: string } {
  try {
    return JSON.parse(localStorage.getItem(OR_KEY) ?? '{}') as { account?: string; model?: string };
  } catch {
    return {};
  }
}

function rememberOr(account: string, model: string) {
  try {
    localStorage.setItem(OR_KEY, JSON.stringify({ account, model }));
  } catch {
    /* just not remembered */
  }
}

export class NewSessionSheet extends Sheet {
  private presets: PresetInfo[] = [];
  private groups: GroupInfo[] = [];
  private recents: RecentDir[] = [];
  private folder: Folder | null = null;
  /** The preset name to start, or SHELL for a plain shell. */
  private choice = SHELL;
  private skip = false;
  /** Walking down the tree by tapping, rather than choosing from a list. */
  private browsing = false;
  private listing: BrowseResult | null = null;
  private seq = 0;
  private timer = 0;

  private input = textInput({ mono: true, cls: 'flex-1' });
  private suggestions = el('div', 'ns-list');
  private crumbs = el('div', 'ns-crumbs');
  private agents = el('div', 'ns-seg');
  private moreRow = el('div', 'ns-more');
  private skipRow = el('label', 'ns-skip');
  private skipBox = el('input');
  private skipText = el('span');
  private groupBox = el('div', 'ns-groups');
  private err = el('div', 'ns-err');
  private go = button({ cls: 'w-full font-bold' }, 'Start');
  private showMore = false;
  /** OpenRouter accounts and the choices made for an OpenRouter session. */
  private accounts: Account[] = [];
  private orAccount = '';
  private orModel = '';
  /** '' lets the server pick the CLI from the model. */
  private orDriver: '' | Driver = '';
  private orCap = '';
  private orBox = el('div', 'flex flex-col gap-2');
  /** There is an account with pinned models to chat with. */
  private chatAccounts = false;

  constructor(host: HTMLElement, private signedOut: () => void, private hooks: NewSessionHooks) {
    super(host, '', 'New session', signedOut);
  }

  /** Open the sheet. Make a new sheet for every opening. `dir` is the folder to start in, when the caller knows one (a project, the focused pane). */
  async open(dir?: string, agent?: 'claude' | 'codex') {
    if (!this.gone) return;
    const cached = cachedNewSessionData();
    if (cached) {
      // Fill the sheet before it is shown, so it slides in at its final height. Fresh data trickles in afterwards.
      this.apply(cached, dir, agent);
      this.show();
      this.focusFolder();
      void loadNewSessionData().then((fresh) => this.refresh(fresh));
      void this.loadAccounts();
      return;
    }
    this.body.replaceChildren(sheetMsg('Loading…'));
    this.body.style.minHeight = '20rem';
    this.show();
    const data = await loadNewSessionData();
    if (this.gone) return;
    this.body.style.minHeight = '';
    this.apply(data, dir, agent);
    this.focusFolder();
    void this.loadAccounts();
  }

  /** OpenRouter accounts decide whether the OpenRouter choice exists; they arrive after the sheet is up. */
  private async loadAccounts() {
    const r = await getJson<Account[]>('/api/accounts', this.signedOut);
    if (this.gone) return;
    this.accounts = r.ok ? orAccounts(r.data) : [];
    this.chatAccounts = r.ok && r.data.some((a) => !isWeb(a.kind) && a.hasKey && a.models.length > 0);
    if (this.choice === OR_CHOICE && this.accounts.length === 0) this.choice = this.fallbackChoice();
    this.syncOrPick();
    this.syncAgents();
    this.syncSkip();
    this.syncOr();
  }

  /** What to start when the remembered choice cannot be honoured. */
  private fallbackChoice(): string {
    const { claude, codex } = mainAgents(this.presets);
    return (claude ?? codex)?.name ?? SHELL;
  }

  /** Keep the chosen account and model valid for the accounts there are now. */
  private syncOrPick() {
    const saved = rememberedOr();
    const account = this.accounts.find((a) => a.id === (this.orAccount || saved.account)) ?? this.accounts[0];
    this.orAccount = account?.id ?? '';
    const models = account?.models ?? [];
    this.orModel = models.find((m) => m.id === (this.orModel || saved.model))?.id ?? models[0]?.id ?? '';
  }

  private apply(data: NewSessionData, dir?: string, agent?: 'claude' | 'codex') {
    ({ presets: this.presets, groups: this.groups, recents: this.recents } = data);

    const names = new Set(this.presets.map((p) => p.name));
    const saved = remembered();
    const { claude, codex } = mainAgents(this.presets);
    this.choice = saved !== null && (saved === SHELL || saved === OR_CHOICE || names.has(saved)) ? saved : (claude ?? codex)?.name ?? SHELL;
    // A home-screen shortcut ("New Codex session") names the agent; otherwise the last choice stands.
    const wanted = agent ? { claude, codex }[agent] : undefined;
    if (wanted) this.choice = wanted.name;
    this.skip = false;
    this.browsing = false;
    this.showMore = false;
    this.listing = null;

    const start = dir ? this.recents.find((d) => d.path === dir) : this.recents[0];
    this.folder = dir ? { path: dir, short: start?.short ?? dir } : start ? { path: start.path, short: start.short } : null;
    this.build();
    if (this.folder) this.input.value = this.folder.short;
    this.syncAll();
    this.showFolders();
  }

  /** Newer lists than the ones the sheet opened with: swap them in without disturbing what the user has done. */
  private refresh(fresh: NewSessionData) {
    if (this.gone) return;
    const same = JSON.stringify(fresh) === JSON.stringify({ presets: this.presets, groups: this.groups, recents: this.recents });
    if (same) return;
    ({ presets: this.presets, groups: this.groups, recents: this.recents } = fresh);
    if (this.groups.length > 0) this.err.before(this.groupBox);
    else this.groupBox.remove();
    this.syncAgents();
    this.syncGroups();
    if (!this.browsing && !this.input.value.trim()) this.showFolders();
  }

  /** A mouse user types at once; on a touch screen the keyboard would cover the sheet, so wait for a tap on the field. */
  private focusFolder() {
    if (!window.matchMedia('(pointer: fine)').matches) return;
    this.input.focus({ preventScroll: true });
    this.input.select();
  }

  protected override onClose() {
    window.clearTimeout(this.timer);
    this.seq++;
  }

  // ------------------------------------------------------------------ building

  private build() {
    this.input.type = 'text';
    this.input.placeholder = '~/code/my-project';
    this.input.autocomplete = 'off';
    this.input.autocapitalize = 'off';
    this.input.spellcheck = false;
    this.input.setAttribute('aria-label', 'Folder');
    this.input.enterKeyHint = 'go';
    this.input.addEventListener('input', () => this.typed());
    this.input.addEventListener('focus', () => {
      if (!this.browsing && !this.input.value.trim()) this.showFolders();
    });
    this.input.addEventListener('keydown', (e) => {
      if (e.key !== 'Enter') return;
      e.preventDefault();
      void this.confirmTyped();
    });

    const browse = button({ variant: 'secondary', size: 'md', cls: 'aria-pressed:ring-2 aria-pressed:ring-primary' }, icon('folder'), 'Browse');
    browse.setAttribute('aria-pressed', 'false');
    browse.addEventListener('click', () => {
      this.browsing = !this.browsing;
      browse.setAttribute('aria-pressed', String(this.browsing));
      if (this.browsing) void this.browseAt(this.folder?.short ?? '~');
      else {
        this.crumbs.replaceChildren();
        this.showFolders();
      }
    });
    const folderRow = el('div', 'flex gap-2');
    folderRow.append(this.input, browse);

    this.skipBox.type = 'checkbox';
    this.skipBox.addEventListener('change', () => {
      this.skip = this.skipBox.checked;
    });
    this.skipRow.append(this.skipBox, this.skipText);

    this.go.addEventListener('click', () => void this.start());

    const folderLabel = sheetLabel('Folder');
    const startLabel = sheetLabel('Start');
    this.body.replaceChildren(folderLabel, folderRow, this.crumbs, this.suggestions, startLabel, this.agents, this.moreRow, this.orBox, this.skipRow);
    if (this.groups.length > 0) this.body.append(this.groupBox);
    this.body.append(this.err, this.go);
    // Enter anywhere but a button or the folder field starts the session.
    this.body.addEventListener('keydown', (e) => {
      if (e.key !== 'Enter' || e.target === this.input || (e.target instanceof HTMLElement && e.target.closest('button'))) return;
      e.preventDefault();
      void this.start();
    });
  }

  private syncAll() {
    this.syncAgents();
    this.syncSkip();
    this.syncOr();
    this.syncGroups();
    this.syncGo();
  }

  private preset(name: string): PresetInfo | undefined {
    return this.presets.find((p) => p.name === name);
  }

  private syncAgents() {
    const { claude, codex, more } = mainAgents(this.presets);
    const seg = (label: string, name: string) => {
      const b = el('button', 'ns-opt', label);
      b.type = 'button';
      b.setAttribute('role', 'radio');
      const on = this.choice === name;
      b.setAttribute('aria-checked', String(on));
      b.classList.toggle('on', on);
      b.addEventListener('click', () => this.pick(name));
      return b;
    };
    const items: HTMLElement[] = [];
    if (claude) items.push(seg('Claude', claude.name));
    if (codex) items.push(seg('Codex', codex.name));
    if (this.accounts.length > 0) items.push(seg('OpenRouter', OR_CHOICE));
    if (this.chatAccounts) items.push(seg('Chat', CHAT_CHOICE));
    items.push(seg('Shell', SHELL));
    const chosenMore = more.some((p) => p.name === this.choice);
    if (more.length > 0) {
      const b = el('button', chosenMore ? 'ns-opt on' : 'ns-opt', chosenMore ? this.choice : 'More…');
      b.type = 'button';
      b.setAttribute('aria-expanded', String(this.showMore));
      b.addEventListener('click', () => {
        this.showMore = !this.showMore;
        this.syncAgents();
      });
      items.push(b);
    }
    this.agents.setAttribute('role', 'radiogroup');
    this.agents.setAttribute('aria-label', 'What to start');
    this.agents.replaceChildren(...items);
    this.moreRow.replaceChildren();
    this.moreRow.hidden = !this.showMore || more.length === 0;
    if (!this.moreRow.hidden) {
      for (const p of more) {
        const b = chip({ on: this.choice === p.name }, p.name);
        if (p.command) b.title = p.command;
        b.addEventListener('click', () => this.pick(p.name));
        this.moreRow.append(b);
      }
    }
  }

  private pick(name: string) {
    if (name === CHAT_CHOICE) {
      this.close();
      void new DirectChatSheet(this.host, this.signedOut).open();
      return;
    }
    this.choice = name;
    remember(name);
    if (!this.preset(name)?.yolo) this.skip = false;
    this.syncAgents();
    this.syncSkip();
    this.syncOr();
  }

  /** The skip-permissions box only exists for Claude and Codex, and is locked on for a preset that always skips. */
  private syncSkip() {
    if (this.choice === OR_CHOICE) {
      this.skipRow.hidden = false;
      this.skipBox.checked = this.skip;
      this.skipBox.disabled = false;
      this.skipRow.classList.toggle('risky', this.skip);
      this.skipText.textContent = this.orDriverNow() === 'codex' ? 'Skip permission checks (--yolo)' : 'Skip permission checks (--dangerously-skip-permissions)';
      return;
    }
    const p = this.preset(this.choice);
    this.skipRow.hidden = !p || !canSkip(p);
    if (!p) return;
    const locked = p.yolo === true;
    this.skipBox.checked = locked || this.skip;
    this.skipBox.disabled = locked;
    this.skipRow.classList.toggle('risky', this.skipBox.checked);
    this.skipText.replaceChildren(
      ...(locked ? [icon('shield-alert', 'size-4 shrink-0')] : []),
      locked
        ? 'Permission checks are always skipped for this preset'
        : p.agent === 'codex'
          ? 'Skip permission checks (--yolo)'
          : 'Skip permission checks (--dangerously-skip-permissions)',
    );
  }

  /** The CLI an OpenRouter session will run in: the one asked for, else the one the model calls for. */
  private orDriverNow(): Driver {
    return this.orDriver || driverFor(this.orModel);
  }

  /** The account, model, CLI and cap for an OpenRouter session. */
  private syncOr() {
    this.orBox.hidden = this.choice !== OR_CHOICE;
    if (this.orBox.hidden) return;
    const account = this.accounts.find((a) => a.id === this.orAccount);
    const parts: HTMLElement[] = [];
    if (this.accounts.length > 1) {
      const acct = select(this.accounts.map((a) => ({ value: a.id, label: a.name })), { value: this.orAccount, label: 'OpenRouter account' });
      acct.addEventListener('change', () => {
        this.orAccount = acct.value;
        this.orModel = '';
        this.syncOrPick();
        this.syncOr();
      });
      parts.push(acct);
    }
    const models = account?.models ?? [];
    if (models.length === 0) {
      parts.push(el('div', 'text-sm text-muted-foreground', 'No models pinned on this account. Pin some in Settings, under Account.'));
    } else {
      const model = select(models.map((m) => ({ value: m.id, label: `${shortModel(m.id)} · ${priceLine(m)}` })), { value: this.orModel, label: 'Model' });
      model.addEventListener('change', () => {
        this.orModel = model.value;
        rememberOr(this.orAccount, this.orModel);
        this.syncSkip();
        this.syncOr();
      });
      parts.push(model);
      const driver = select(
        [{ value: '', label: `Run in ${DRIVER_LABEL[driverFor(this.orModel)]} (automatic)` }, ...(['claude', 'codex'] as const).map((d) => ({ value: d, label: `Run in ${DRIVER_LABEL[d]}` }))],
        { value: this.orDriver, label: 'Which agent runs it' },
      );
      driver.addEventListener('change', () => {
        this.orDriver = driver.value as '' | Driver;
        this.syncSkip();
      });
      const cap = textInput({ placeholder: 'Dollar cap (optional)', label: 'Dollar cap', cls: 'flex-1' });
      cap.inputMode = 'decimal';
      cap.value = this.orCap;
      cap.addEventListener('input', () => (this.orCap = cap.value));
      const row = el('div', 'flex gap-2');
      row.append(driver, cap);
      parts.push(row);
    }
    this.orBox.replaceChildren(...parts);
    this.syncGo();
  }

  private syncGroups() {
    if (this.groups.length === 0) return;
    this.groupBox.replaceChildren(sheetLabel('Or start a group'));
    const row = el('div', 'flex flex-wrap gap-2');
    for (const g of this.groups) {
      const b = chip({ cls: 'border border-dashed border-input bg-transparent' }, `▶ ${g.name}`);
      b.title = `Start together: ${g.presets.join(', ')}`;
      b.addEventListener('click', () => void this.startGroup(g.name));
      row.append(b);
    }
    this.groupBox.append(row);
  }

  private syncGo() {
    this.go.disabled = this.busy || !this.folder || (this.choice === OR_CHOICE && !this.orModel);
    this.go.textContent = this.busy ? 'Starting…' : 'Start';
  }

  // ------------------------------------------------------------------ the folder

  private setFolder(f: Folder) {
    this.folder = f;
    this.input.value = f.short;
    this.err.textContent = '';
    this.syncGo();
  }

  /** The pinned projects, then the recent ones, each once. */
  private offered(): Folder[] {
    const out: Folder[] = [];
    const seen = new Set<string>();
    const add = (path: string, short: string) => {
      if (seen.has(path)) return;
      seen.add(path);
      out.push({ path, short });
    };
    for (const p of this.hooks.pinned()) add(p, this.recents.find((d) => d.path === p)?.short ?? p);
    for (const d of this.recents) add(d.path, d.short);
    return out;
  }

  private folderRow(f: Folder, name: string, opts: { pinned?: boolean; drill?: boolean } = {}): HTMLElement {
    const b = el('button', this.folder?.path === f.path ? 'ns-row on' : 'ns-row');
    b.type = 'button';
    const text = el('span', 'ns-rowtext');
    text.append(el('span', 'ns-name', `${opts.pinned ? '★ ' : ''}${name}`), el('span', 'ns-path', f.short));
    b.append(icon('folder'), text);
    if (opts.drill) b.append(icon('chevron', 'icon-svg ns-go-in'));
    b.addEventListener('click', () => {
      this.setFolder(f);
      if (this.browsing) void this.browseAt(f.short);
      else this.suggestions.replaceChildren();
    });
    return b;
  }

  /** Pinned and recent projects, shown while nothing is typed. */
  private showFolders() {
    this.seq++;
    const pinned = new Set(this.hooks.pinned());
    const rows = this.offered().map((f) =>
      this.folderRow(f, this.recents.find((d) => d.path === f.path)?.name ?? f.short.split('/').filter(Boolean).pop() ?? f.short, { pinned: pinned.has(f.path) }),
    );
    if (rows.length === 0) rows.push(el('div', 'ns-empty', 'No recent folders yet: type a path, or Browse'));
    this.suggestions.replaceChildren(el('div', 'ns-sub', 'Projects'), ...rows);
  }

  private typed() {
    this.folder = null;
    this.syncGo();
    window.clearTimeout(this.timer);
    const text = this.input.value.trim();
    if (!text) {
      this.listing = null;
      if (!this.browsing) this.showFolders();
      return;
    }
    this.timer = window.setTimeout(() => void this.suggest(text), 120);
  }

  /** The folders that fit what is typed. A path that ends in a slash is itself a folder you can use. */
  private async suggest(text: string) {
    const mine = ++this.seq;
    const l = await browseDirs(text);
    if (mine !== this.seq || this.gone) return;
    this.listing = l;
    const rows: HTMLElement[] = [];
    if (l.path && (text.endsWith('/') || text === '~')) {
      rows.push(this.folderRow({ path: l.path, short: l.dir }, `Use ${l.dir}`));
    }
    for (const e of l.entries) rows.push(this.folderRow({ path: e.path, short: e.short }, e.name, { drill: this.browsing }));
    if (rows.length === 0) rows.push(el('div', 'ns-empty', 'No folder like that'));
    this.suggestions.replaceChildren(...rows);
  }

  /** Enter in the folder field: settle on the folder, never start from here. */
  private async confirmTyped() {
    const text = this.input.value.trim();
    if (this.folder && this.folder.short === text) {
      this.go.focus({ preventScroll: true });
      return;
    }
    if (!text) return;
    window.clearTimeout(this.timer);
    const l = await browseDirs(text);
    if (this.gone) return;
    const whole = l.path && (text.endsWith('/') || text === '~');
    const first = l.entries[0];
    if (whole) this.setFolder({ path: l.path, short: l.dir });
    else if (first) this.setFolder({ path: first.path, short: first.short });
    else {
      this.err.textContent = 'No folder like that';
      return;
    }
    if (this.browsing && this.folder) await this.browseAt(this.folder.short);
    else this.suggestions.replaceChildren();
    this.go.focus({ preventScroll: true });
  }

  /** Browsing: show the subfolders of `short`, with the way back up as a trail of buttons. */
  private async browseAt(short: string) {
    const mine = ++this.seq;
    const l = await browseDirs(short.endsWith('/') || short === '~' ? short : `${short}/`);
    if (mine !== this.seq || this.gone) return;
    if (!l.path) {
      this.err.textContent = 'That folder is not available';
      return;
    }
    this.listing = l;
    this.setFolder({ path: l.path, short: l.dir });
    const parts = l.dir === '~' ? ['~'] : l.dir.startsWith('~/') ? ['~', ...l.dir.slice(2).split('/')] : ['/', ...l.dir.split('/').filter(Boolean)];
    const trail: HTMLElement[] = [];
    parts.forEach((part, i) => {
      const target = parts[0] === '~' ? parts.slice(0, i + 1).join('/') : `/${parts.slice(1, i + 1).join('/')}`;
      const b = el('button', i === parts.length - 1 ? 'ns-crumb on' : 'ns-crumb', part);
      b.type = 'button';
      b.addEventListener('click', () => void this.browseAt(target));
      if (i > 0) trail.push(el('span', 'ns-sep', '/'));
      trail.push(b);
    });
    this.crumbs.replaceChildren(...trail);
    const rows = l.entries.map((e) => this.folderRow({ path: e.path, short: e.short }, e.name, { drill: true }));
    if (rows.length === 0) rows.push(el('div', 'ns-empty', 'No subfolders'));
    this.suggestions.replaceChildren(...rows);
  }

  // ------------------------------------------------------------------ going

  private async start() {
    if (this.busy) return;
    // A folder typed but not yet settled on: settle first, so Enter never starts somewhere unseen.
    if (!this.folder) return void (await this.confirmTyped());
    const p = this.preset(this.choice);
    const viaOr = this.choice === OR_CHOICE;
    let cap: number | undefined;
    if (viaOr && this.orCap.trim() !== '') {
      cap = Number(this.orCap.trim().replace(/^\$/, ''));
      if (!Number.isFinite(cap) || cap <= 0) return void (this.err.textContent = 'The dollar cap must be more than zero');
    }
    const yolo = viaOr ? this.skip : !!p && canSkip(p) && (this.skip || p.yolo === true);
    this.busy = true;
    this.err.textContent = '';
    this.syncGo();
    const r = await launchSession({
      preset: viaOr ? undefined : p?.name,
      yolo,
      dir: this.folder.path,
      openrouter: viaOr ? { account: this.orAccount, model: this.orModel, driver: this.orDriver || undefined, cap_usd: cap } : undefined,
    });
    this.busy = false;
    if (this.gone) return;
    if (!r.ok) {
      if (r.signedOut) {
        this.close();
        return this.signedOut();
      }
      this.err.textContent = r.error;
      this.syncGo();
      return;
    }
    this.close();
    this.hooks.opened(r.id);
  }

  private async startGroup(name: string) {
    if (this.busy) return;
    this.busy = true;
    this.syncGo();
    const r = await launchGroup(name);
    this.busy = false;
    if (this.gone) return;
    if (!r.ok) {
      if (r.signedOut) {
        this.close();
        return this.signedOut();
      }
      this.err.textContent = r.error;
      this.syncGo();
      return;
    }
    this.close();
    if (r.ids[0]) this.hooks.opened(r.ids[0]);
  }
}
