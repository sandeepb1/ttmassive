// Provider accounts in Settings: the list, the editor for one account, and the picker that pins
// models from OpenRouter's catalog or from an endpoint's own list. Keys go to the server and
// never come back; the list only ever knows the last four characters.

import { callJson, getJson } from './api-json';
import {
  KIND_LABEL,
  SORTS,
  SORT_CHOICES,
  ageLabel,
  balanceLine,
  contextLabel,
  emptyInput,
  filterModels,
  inputOf,
  parsePrice,
  pinFromCatalog,
  pinFromId,
  priceLine,
  providersOf,
  sortModels,
  togglePin,
  type Account,
  type AccountInput,
  type AccountKind,
  type Catalog,
  type CatalogModel,
  type PinnedModel,
  type Prefs,
  type Remote,
  type SortKey,
} from './accounts-model';
import { el } from './dom';
import { Sheet } from './sheet';
import { button } from './ui/button';
import { chip, setChipOn } from './ui/chip';
import { input, select } from './ui/input';
import { row, setRowCurrent } from './ui/row';
import { sheetActions, sheetBlock, sheetHint, sheetLabel, sheetMsg } from './ui/sheet-parts';

/** Rows drawn at once in a picker; the catalog has hundreds. */
const PAGE = 60;

const ACCOUNTS = '/api/accounts';

/** Models to choose from, with how they should be offered. */
interface PickSource {
  title: string;
  models: CatalogModel[];
  /** Catalog entries have prices, age and tool flags worth sorting and filtering on. */
  rich: boolean;
  refresh?: () => Promise<CatalogModel[] | null>;
}

/** The accounts block for Settings. Call `reload` after anything that could have changed them. */
export function accountsBlock(host: HTMLElement, onUnauthorized: () => void): HTMLElement {
  const root = sheetBlock();
  const balances = new Map<string, string>();

  const load = async () => {
    const r = await getJson<Account[]>(ACCOUNTS, onUnauthorized);
    if (!r.ok) {
      root.replaceChildren(sheetLabel('Provider accounts'), sheetMsg(r.error, true));
      return;
    }
    render(r.data);
    for (const a of r.data) {
      if (a.kind !== 'openrouter' || !a.hasKey) continue;
      void getJson<Remote>(`${ACCOUNTS}/${encodeURIComponent(a.id)}/remote`, onUnauthorized).then((b) => {
        const line = b.ok ? balanceLine(b.data) : null;
        if (line && balances.get(a.id) !== line) {
          balances.set(a.id, line);
          render(r.data);
        }
      });
    }
  };

  const edit = (a: Account | null, kind: AccountKind) => new AccountSheet(host, onUnauthorized, a, kind, () => void load()).open();

  const render = (accounts: Account[]) => {
    const list = sheetBlock();
    for (const a of accounts) {
      const bits = [a.hasKey ? `key …${a.last4}` : 'no key', `${a.models.length} model${a.models.length === 1 ? '' : 's'}`];
      const bal = balances.get(a.id);
      if (bal) bits.push(bal);
      if (!a.useInOrchestrator) bits.push('not in flows');
      const r = row(a.name, { hint: bits.join(' · '), mark: KIND_LABEL[a.kind] });
      r.addEventListener('click', () => edit(a, a.kind));
      list.append(r);
    }
    if (!accounts.length) list.append(sheetHint('No accounts yet. Add an OpenRouter or OpenAI-compatible account to use its models.'));
    const add = el('div', 'flex flex-wrap gap-2');
    add.append(
      button({ variant: 'secondary', size: 'sm', onClick: () => edit(null, 'openrouter') }, 'Add OpenRouter'),
      button({ variant: 'secondary', size: 'sm', onClick: () => edit(null, 'openai-compatible') }, 'Add OpenAI-compatible'),
    );
    root.replaceChildren(sheetLabel('Provider accounts'), list, add);
  };

  root.replaceChildren(sheetLabel('Provider accounts'), sheetHint('Loading…'));
  void load();
  return root;
}

class AccountSheet extends Sheet {
  private draft: AccountInput;
  private keyField = input({ type: 'password', mono: true, label: 'API key' });
  private error = el('div', 'min-h-5 text-sm text-danger-text');
  private models = sheetBlock();

  constructor(
    host: HTMLElement,
    private unauthorized: () => void,
    private existing: Account | null,
    kind: AccountKind,
    private done: () => void,
  ) {
    super(host, '', existing ? existing.name : `Add ${KIND_LABEL[kind]}`, unauthorized);
    this.draft = existing ? inputOf(existing) : emptyInput(kind);
  }

  open() {
    this.tall();
    const d = this.draft;
    const name = input({ value: d.name, placeholder: d.kind === 'openrouter' ? 'OpenRouter' : 'Haimaker', label: 'Account name' });
    name.addEventListener('input', () => (d.name = name.value));

    const parts: HTMLElement[] = [sheetBlock(sheetLabel('Name'), name)];

    if (d.kind === 'openai-compatible') {
      const url = input({ value: d.baseUrl, placeholder: 'https://api.example.com/v1', mono: true, label: 'Base URL' });
      url.addEventListener('input', () => (d.baseUrl = url.value));
      parts.push(sheetBlock(sheetLabel('Base URL'), url, sheetHint('Where /chat/completions and /models live, usually ending in /v1.')));
    }

    this.keyField.autocomplete = 'off';
    this.keyField.placeholder = this.existing?.hasKey ? `Saved, ending ${this.existing.last4}. Leave blank to keep it` : 'API key';
    parts.push(sheetBlock(sheetLabel('API key'), this.keyField, sheetHint('Stored on this server, readable only by you. It is never sent back to the page.')));

    const flows = chip({ on: d.useInOrchestrator }, d.useInOrchestrator ? 'On' : 'Off');
    flows.addEventListener('click', () => {
      d.useInOrchestrator = !d.useInOrchestrator;
      flows.textContent = d.useInOrchestrator ? 'On' : 'Off';
      setChipOn(flows, d.useInOrchestrator);
    });
    const flowRow = el('div', 'flex items-center justify-between gap-3');
    flowRow.append(el('span', 'text-md', 'Use in orchestrator flows'), flows);
    parts.push(sheetBlock(flowRow, sheetHint('Off hides this account’s models from flow pickers, Suggest and Ask AI, and automatic model choice. Single sessions can still use it.')));

    if (d.kind === 'openrouter') parts.push(this.routingBlock());
    parts.push(this.models);
    this.renderModels();

    const save = button({ onClick: () => void this.save(save) }, 'Save');
    const actions = [save];
    if (this.existing) actions.unshift(button({ variant: 'soft-danger', onClick: () => void this.remove() }, 'Delete'));
    parts.push(this.error, sheetActions(...actions));

    const wrap = el('div', 'flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto pb-2');
    wrap.append(...parts);
    this.body.replaceChildren(wrap);
    this.show();
  }

  /** Where OpenRouter may send this account's requests. */
  private routingBlock(): HTMLElement {
    const p = this.draft.prefs;
    const sort = select(SORT_CHOICES, { value: p.sort ?? '', label: 'Prefer', cls: 'w-auto' });
    sort.addEventListener('change', () => (p.sort = (sort.value || null) as Prefs['sort']));
    const flag = (label: string, key: 'denyDataCollection' | 'zdr' | 'allowFallbacks') => {
      const c = chip({ on: p[key] }, p[key] ? 'On' : 'Off');
      c.addEventListener('click', () => {
        p[key] = !p[key];
        c.textContent = p[key] ? 'On' : 'Off';
        setChipOn(c, p[key]);
      });
      const r = el('div', 'flex items-center justify-between gap-3');
      r.append(el('span', 'text-md', label), c);
      return r;
    };
    const pick = el('div', 'flex items-center justify-between gap-3');
    pick.append(el('span', 'text-md', 'Prefer'), sort);
    return sheetBlock(
      sheetLabel('Routing'),
      pick,
      flag('Only providers that keep no prompts', 'denyDataCollection'),
      flag('Zero data retention only', 'zdr'),
      flag('Fall back to another provider', 'allowFallbacks'),
      sheetHint('Applies to every request this account makes, in sessions and in flows. Stricter settings can leave fewer providers for a model.'),
    );
  }

  private renderModels() {
    const d = this.draft;
    const openrouter = d.kind === 'openrouter';
    const head = el('div', 'flex flex-wrap items-center gap-2');
    head.append(
      button({ variant: 'secondary', size: 'sm', onClick: () => void (openrouter ? this.browseCatalog() : this.fetchEndpoint()) }, openrouter ? 'Browse catalog' : 'Fetch from endpoint'),
    );
    const manual = input({ placeholder: 'Add a model id', mono: true, label: 'Model id', cls: 'min-w-0 flex-1' });
    const add = () => {
      const id = manual.value.trim();
      if (!id || d.models.some((m) => m.id === id)) return;
      d.models = [...d.models, pinFromId(id)];
      manual.value = '';
      this.renderModels();
    };
    manual.addEventListener('keydown', (e) => e.key === 'Enter' && add());
    const addRow = el('div', 'flex gap-2');
    addRow.append(manual, button({ variant: 'secondary', onClick: add }, 'Add'));

    const list = sheetBlock();
    for (const m of d.models) list.append(this.modelRow(m, !openrouter));
    if (!d.models.length) list.append(sheetHint('No models pinned. Pinned models are the shortlist offered in pickers.'));
    this.models.replaceChildren(sheetLabel(`Pinned models (${d.models.length})`), head, addRow, list);
  }

  private modelRow(m: PinnedModel, editablePrice: boolean): HTMLElement {
    const r = el('div', 'flex flex-col gap-1.5 rounded-lg bg-muted px-4 py-3');
    const top = el('div', 'flex items-center justify-between gap-3');
    top.append(
      el('span', 'min-w-0 flex-1 break-all font-mono text-sm', m.id),
      button({ variant: 'ghost', size: 'sm', label: `Unpin ${m.id}`, onClick: () => {
        this.draft.models = this.draft.models.filter((x) => x.id !== m.id);
        this.renderModels();
      } }, 'Remove'),
    );
    r.append(top);
    if (!editablePrice) {
      r.append(el('span', 'text-sm text-muted-foreground', `${priceLine(m)}${m.tools ? '' : ' · no tools'}`));
      return r;
    }
    const fields = el('div', 'flex flex-wrap items-center gap-2');
    const price = (label: string, get: () => number | null, set: (v: number | null) => void) => {
      const f = input({ value: get() === null ? '' : String(get()), placeholder: '$ / 1M', label, cls: 'h-9 w-28' });
      f.inputMode = 'decimal';
      f.addEventListener('input', () => {
        const p = parsePrice(f.value);
        f.setAttribute('aria-invalid', String(p === 'bad'));
        if (p !== 'bad') set(p);
      });
      return f;
    };
    const tools = chip({ on: m.tools }, m.tools ? 'Tools' : 'No tools');
    tools.addEventListener('click', () => {
      m.tools = !m.tools;
      tools.textContent = m.tools ? 'Tools' : 'No tools';
      setChipOn(tools, m.tools);
    });
    fields.append(
      el('span', 'text-xs text-muted-foreground', 'In'),
      price(`${m.id} input price`, () => m.priceIn, (v) => (m.priceIn = v)),
      el('span', 'text-xs text-muted-foreground', 'Out'),
      price(`${m.id} output price`, () => m.priceOut, (v) => (m.priceOut = v)),
      tools,
    );
    r.append(fields, el('span', 'text-xs text-muted-foreground', 'Optional. Without a price this account shows tokens only, and dollar caps do not apply to it.'));
    return r;
  }

  private pick(source: PickSource) {
    new ModelPicker(this.host, this.unauthorized, source, this.draft.models, (next) => {
      this.draft.models = next;
      this.renderModels();
    }).open();
  }

  private async browseCatalog() {
    const load = async () => {
      const r = await getJson<Catalog>('/api/openrouter/models', this.unauthorized);
      return r.ok ? r.data.models : null;
    };
    const models = await load();
    if (!models) return this.fail('Could not load the OpenRouter catalog');
    this.fail('');
    this.pick({ title: 'OpenRouter models', models, rich: true, refresh: async () => {
      const r = await getJson<Catalog>('/api/openrouter/models?refresh=1', this.unauthorized);
      return r.ok ? r.data.models : null;
    } });
  }

  private async fetchEndpoint() {
    if (!this.existing) return this.fail('Save the account first, then fetch its models');
    const r = await getJson<Remote>(`${ACCOUNTS}/${encodeURIComponent(this.existing.id)}/remote`, this.unauthorized);
    if (!r.ok) return this.fail(r.error);
    if (!r.data.models.length) return this.fail('The endpoint listed no models. Add them by id instead');
    this.fail('');
    const models: CatalogModel[] = r.data.models.map((id) => ({
      id, name: id, created: 0, description: '', context: 0, priceIn: null, priceOut: null, priceCacheRead: null, tools: true, reasoning: false, structured: false, imageIn: false,
    }));
    this.pick({ title: `${this.existing.name} models`, models, rich: false });
  }

  private fail(text: string) {
    this.error.textContent = text;
  }

  private async save(btn: HTMLButtonElement) {
    const d = this.draft;
    const body: AccountInput = { ...d, name: d.name.trim() };
    const key = this.keyField.value.trim();
    if (key) body.key = key;
    if (!body.name) return this.fail('Give the account a name');
    btn.disabled = true;
    const url = this.existing ? `${ACCOUNTS}/${encodeURIComponent(this.existing.id)}` : ACCOUNTS;
    const r = await callJson<Account>(
      url,
      { method: this.existing ? 'PUT' : 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) },
      this.unauthorized,
    );
    btn.disabled = false;
    if (!r.ok) return this.fail(r.error);
    this.done();
    this.close();
  }

  private async remove() {
    if (!this.existing) return;
    if (!window.confirm(`Delete “${this.existing.name}” and its saved key?`)) return;
    const r = await callJson<unknown>(`${ACCOUNTS}/${encodeURIComponent(this.existing.id)}`, { method: 'DELETE' }, this.unauthorized).catch(() => null);
    // A DELETE answers 204 with no body, which callJson reports as unreadable; the status says what happened.
    if (r && !r.ok && r.status !== 204 && r.status !== 200) return this.fail(r.error);
    this.done();
    this.close();
  }
}

/** Search, sort and filter a long model list; tapping a model pins or unpins it. */
class ModelPicker extends Sheet {
  private sort: SortKey = 'newest';
  private query = '';
  private provider = '';
  private toolsOnly = true;
  private shown = PAGE;
  private list = el('div', 'flex flex-col gap-1.5');
  private count = sheetHint('');
  private pinned: PinnedModel[];
  private currentMatches = 0;

  constructor(
    host: HTMLElement,
    unauthorized: () => void,
    private source: PickSource,
    pinned: PinnedModel[],
    private change: (next: PinnedModel[]) => void,
  ) {
    super(host, '', source.title, unauthorized);
    this.pinned = pinned.map((m) => ({ ...m }));
    if (!source.rich) this.sort = 'name';
  }

  open() {
    this.tall();
    const s = this.source;
    const bar = el('div', 'flex flex-col gap-2');
    const search = input({ placeholder: 'Search models', label: 'Search models' });
    search.addEventListener('input', () => {
      this.query = search.value;
      this.shown = PAGE;
      this.draw();
    });
    bar.append(search);

    const controls = el('div', 'flex flex-wrap items-center gap-2');
    if (s.rich) {
      const sort = select(SORTS, { value: this.sort, label: 'Sort by', cls: 'w-auto' });
      sort.addEventListener('change', () => {
        this.sort = sort.value as SortKey;
        this.shown = PAGE;
        this.draw();
      });
      const vendors = providersOf(s.models);
      const vendor = select([{ value: '', label: 'All vendors' }, ...vendors.map((v) => ({ value: v, label: v }))], { value: '', label: 'Vendor', cls: 'w-auto' });
      vendor.addEventListener('change', () => {
        this.provider = vendor.value;
        this.shown = PAGE;
        this.draw();
      });
      const tools = chip({ on: this.toolsOnly }, 'Tools only');
      tools.addEventListener('click', () => {
        this.toolsOnly = !this.toolsOnly;
        setChipOn(tools, this.toolsOnly);
        this.shown = PAGE;
        this.draw();
      });
      controls.append(sort, vendor, tools);
      if (s.refresh) {
        controls.append(button({ variant: 'secondary', size: 'sm', onClick: () => void this.refresh() }, 'Refresh'));
      }
    }
    bar.append(controls, this.count);

    const scroller = el('div', 'min-h-0 flex-1 overflow-y-auto pb-2');
    scroller.append(this.list);
    const wrap = el('div', 'flex min-h-0 flex-1 flex-col gap-3');
    wrap.append(bar, scroller, sheetActions(button({ onClick: () => this.close() }, 'Done')));
    this.body.replaceChildren(wrap);
    this.draw();
    this.show();
  }

  private async refresh() {
    const next = await this.source.refresh?.();
    if (!next) return;
    this.source = { ...this.source, models: next };
    this.draw();
  }

  private draw() {
    const matches = sortModels(
      filterModels(this.source.models, { query: this.query, provider: this.provider, toolsOnly: this.source.rich && this.toolsOnly }),
      this.sort,
    );
    this.currentMatches = matches.length;
    const now = Math.floor(Date.now() / 1000);
    const rows: HTMLElement[] = [];
    for (const m of matches.slice(0, this.shown)) {
      const on = this.pinned.some((p) => p.id === m.id);
      const bits = this.source.rich ? [m.id, priceLine(m), contextLabel(m.context), ageLabel(m.created, now)].filter(Boolean) : [];
      const r = row(m.name, { hint: bits.join(' · ') || undefined, mark: on ? 'Pinned' : '', current: on });
      r.addEventListener('click', () => {
        this.pinned = togglePin(this.pinned, this.source.rich ? pinFromCatalog(m) : pinFromId(m.id));
        this.change(this.pinned);
        const now = this.pinned.some((p) => p.id === m.id);
        setRowCurrent(r, now);
        r.querySelector('.text-accent-text')?.remove();
        if (now) r.firstElementChild?.append(el('span', 'text-xs text-accent-text', 'Pinned'));
        this.drawCount(this.currentMatches);
      });
      rows.push(r);
    }
    if (matches.length > this.shown) {
      rows.push(button({ variant: 'secondary', onClick: () => {
        this.shown += PAGE;
        this.draw();
      } }, `Show more (${matches.length - this.shown} left)`));
    }
    if (!matches.length) rows.push(sheetMsg('No models match'));
    this.list.replaceChildren(...rows);
    this.drawCount(matches.length);
  }

  private drawCount(matching: number) {
    this.count.textContent = `${matching} of ${this.source.models.length} models · ${this.pinned.length} pinned`;
  }
}
