import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_PREFS,
  KIND_LABEL,
  LIST_CREDIT_USD,
  WEB_KINDS,
  creditPriceLabel,
  isWeb,
  moveInOrder,
  parseCreditPrice,
  webBalanceLine,
  webPoolRows,
  withWebOrder,
  withoutWebOrder,
  cleanVendor,
  intelFromText,
  withIntel,
  helperChoices,
  helperValue,
  parseHelperValue,
  withDriver,
  withHelper,
  type AccountSettings,
  driverFor,
  orAccounts,
  shortModel,
  spendLine,
  switchable,
  usd,
  ageLabel,
  balanceLine,
  contextLabel,
  emptyInput,
  filterModels,
  flowModels,
  inputOf,
  parsePrice,
  pinFromCatalog,
  price,
  priceLine,
  providersOf,
  sortModels,
  togglePin,
  type Account,
  type CatalogModel,
} from './accounts-model.ts';

const model = (id: string, o: Partial<CatalogModel> = {}): CatalogModel => ({
  id,
  name: id.split('/')[1] ?? id,
  created: 0,
  description: '',
  context: 0,
  priceIn: null,
  priceOut: null,
  priceCacheRead: null,
  tools: true,
  reasoning: false,
  structured: false,
  imageIn: false,
  ...o,
});

test('prices read the way people say them', () => {
  assert.equal(price(0.3), '$0.30');
  assert.equal(price(2.5), '$2.50');
  assert.equal(price(15), '$15');
  assert.equal(price(0.25), '$0.25');
  assert.equal(price(12.5), '$12.50');
  assert.equal(price(150), '$150');
  assert.equal(price(0), 'free');
  assert.equal(price(null), '?');
  assert.equal(priceLine({ priceIn: 3, priceOut: 15 }), '$3 in · $15 out');
  assert.equal(priceLine({ priceIn: null, priceOut: null }), 'no price');
});

test('context and age labels', () => {
  assert.equal(contextLabel(32000), '32k ctx');
  assert.equal(contextLabel(1_000_000), '1M ctx');
  assert.equal(contextLabel(1_048_576), '1M ctx');
  assert.equal(contextLabel(0), '');
  const now = 1_800_000_000;
  assert.equal(ageLabel(now - 3600, now), 'today');
  assert.equal(ageLabel(now - 3 * 86400, now), '3d ago');
  assert.equal(ageLabel(now - 21 * 86400, now), '3w ago');
  assert.equal(ageLabel(now - 90 * 86400, now), '3mo ago');
  assert.equal(ageLabel(now - 800 * 86400, now), '2y ago');
  assert.equal(ageLabel(0, now), '');
});

test('sorts by newest, cheapest, context and name', () => {
  const ms = [
    model('a/old', { created: 1, priceIn: 5, priceOut: 5, context: 8000 }),
    model('b/new', { created: 9, priceIn: 1, priceOut: 1, context: 4000 }),
    model('c/unpriced', { created: 5, context: 100000 }),
  ];
  assert.deepEqual(sortModels(ms, 'newest').map((m) => m.id), ['b/new', 'c/unpriced', 'a/old']);
  assert.deepEqual(sortModels(ms, 'cheapest').map((m) => m.id), ['b/new', 'a/old', 'c/unpriced'], 'unknown prices go last');
  assert.deepEqual(sortModels(ms, 'context').map((m) => m.id), ['c/unpriced', 'a/old', 'b/new']);
  assert.deepEqual(sortModels(ms, 'name').map((m) => m.id), ['b/new', 'a/old', 'c/unpriced'], 'by display name, not id');
  assert.deepEqual(ms.map((m) => m.id), ['a/old', 'b/new', 'c/unpriced'], 'the input is not reordered');
});

test('filters by words, vendor and tool support', () => {
  const ms = [model('anthropic/claude-sonnet'), model('openai/gpt-x', { tools: false }), model('openai/gpt-y'), model('router')];
  const f = { query: '', provider: '', toolsOnly: false };
  assert.equal(filterModels(ms, f).length, 4);
  assert.deepEqual(filterModels(ms, { ...f, toolsOnly: true }).map((m) => m.id), ['anthropic/claude-sonnet', 'openai/gpt-y', 'router']);
  assert.deepEqual(filterModels(ms, { ...f, provider: 'openai' }).map((m) => m.id), ['openai/gpt-x', 'openai/gpt-y']);
  assert.deepEqual(filterModels(ms, { ...f, query: 'GPT y' }).map((m) => m.id), ['openai/gpt-y'], 'words match in any case');
  assert.deepEqual(filterModels(ms, { ...f, query: 'claude gpt' }), [], 'every word must match');
});

test('vendors are listed most models first', () => {
  const ms = [model('b/1'), model('a/1'), model('a/2'), model('router')];
  assert.deepEqual(providersOf(ms), ['a', 'b']);
});

test('pinning toggles by id', () => {
  const m = pinFromCatalog(model('a/b', { priceIn: 1, priceOut: 2, tools: false }));
  assert.deepEqual(m, { id: 'a/b', priceIn: 1, priceOut: 2, tools: false });
  const once = togglePin([], m);
  assert.equal(once.length, 1);
  assert.deepEqual(togglePin(once, m), []);
});

test('typed prices', () => {
  assert.equal(parsePrice(''), null);
  assert.equal(parsePrice(' $2.5 '), 2.5);
  assert.equal(parsePrice('0'), 0);
  assert.equal(parsePrice('-1'), 'bad');
  assert.equal(parsePrice('abc'), 'bad');
  assert.equal(parsePrice('1e9'), 'bad');
});

test('balance line', () => {
  assert.equal(balanceLine({ usage: 1, limit: 10, remaining: 9 }), '$9.00 left');
  assert.equal(balanceLine({ usage: 1.5, limit: 10, remaining: null }), '$1.50 of $10.00 used');
  assert.equal(balanceLine({ usage: 0, limit: null, remaining: null }), '$0.00 used');
  assert.equal(balanceLine({ usage: null, limit: null, remaining: null }), null);
});

test('a Firecrawl balance is counted in credits, not dollars', () => {
  assert.equal(balanceLine({ usage: 132, limit: 1000, remaining: 868, unit: 'credits' }), '868 of 1000 credits left');
  assert.equal(balanceLine({ usage: null, limit: null, remaining: 40, unit: 'credits' }), '40 credits left');
  assert.equal(balanceLine({ usage: null, limit: null, remaining: null, unit: 'credits' }), null);
});

test('flows only see accounts that opt in and have a key', () => {
  const acct = (id: string, o: Partial<Account>): Account => ({
    id,
    name: id,
    kind: 'openrouter',
    baseUrl: '',
    hasKey: true,
    last4: '1234',
    useInOrchestrator: true,
    models: [{ id: `${id}/m`, priceIn: null, priceOut: null, tools: true }],
    prefs: { ...DEFAULT_PREFS },
    ...o,
  });
  const list = [acct('on', {}), acct('off', { useInOrchestrator: false }), acct('nokey', { hasKey: false })];
  assert.deepEqual(flowModels(list).map((x) => x.model.id), ['on/m']);
});

test('a new account starts with balanced routing and fallbacks on', () => {
  assert.deepEqual(emptyInput('openrouter').prefs, { sort: null, denyDataCollection: false, zdr: false, allowFallbacks: true });
  assert.notEqual(emptyInput('openrouter').prefs, emptyInput('openrouter').prefs, 'each draft has its own copy');
});

test('an edit draft is a copy', () => {
  const a: Account = { id: 'a1', name: 'N', kind: 'openrouter', baseUrl: 'u', hasKey: true, last4: '1', useInOrchestrator: false, models: [{ id: 'x', priceIn: 1, priceOut: 2, tools: true }], prefs: { sort: 'price', denyDataCollection: true, zdr: false, allowFallbacks: false } };
  const draft = inputOf(a);
  draft.models[0]!.priceIn = 99;
  assert.equal(a.models[0]!.priceIn, 1);
  draft.prefs.sort = null;
  assert.equal(a.prefs.sort, 'price', 'prefs are copied too');
  assert.equal(draft.prefs.denyDataCollection, true);
  assert.equal('key' in draft, false, 'the key is never prefilled');
});

test('anthropic models run in Claude Code and the rest in Codex, like the server decides', () => {
  assert.equal(driverFor('anthropic/claude-haiku-5.5'), 'claude');
  assert.equal(driverFor('~anthropic/claude-haiku-latest'), 'claude');
  assert.equal(driverFor('openai/gpt-6-luna'), 'codex');
  assert.equal(driverFor('openrouter/auto'), 'codex');
  assert.equal(driverFor('plain'), 'codex');
});

test('spend is shown in dollars', () => {
  assert.equal(usd(0), '$0.00');
  assert.equal(usd(0.004), '<$0.01');
  assert.equal(usd(0.176), '$0.18');
  assert.equal(usd(12), '$12.00');
  assert.equal(spendLine(0.5, null), '$0.50');
  assert.equal(spendLine(0.5, 2), '$0.50 of $2.00');
  assert.equal(shortModel('anthropic/claude-haiku-5.5'), 'claude-haiku-5.5');
  assert.equal(shortModel('plain'), 'plain');
});

test('only OpenRouter accounts with a key can start a session', () => {
  const a = (id: string, kind: 'openrouter' | 'openai-compatible', hasKey: boolean): Account => ({
    id, name: id, kind, baseUrl: '', hasKey, last4: '', useInOrchestrator: true, models: [], prefs: { ...DEFAULT_PREFS },
  });
  assert.deepEqual(orAccounts([a('a', 'openrouter', true), a('b', 'openrouter', false), a('c', 'openai-compatible', true)]).map((x) => x.id), ['a']);
});

test('a running session can switch to models its CLI can run, and sees the others disabled', () => {
  const pin = (id: string) => ({ id, priceIn: 1, priceOut: 2, tools: true });
  const acct: Account = {
    id: 'a', name: 'A', kind: 'openrouter', baseUrl: '', hasKey: true, last4: '', useInOrchestrator: true, prefs: { ...DEFAULT_PREFS },
    models: [pin('anthropic/claude-sonnet-5.5'), pin('openai/gpt-6-luna'), pin('anthropic/claude-haiku-5.5')],
  };
  const rows = switchable(acct, 'anthropic/claude-haiku-5.5', 'claude');
  assert.deepEqual(rows.map((r) => [r.model.id, r.ok, r.current]), [
    ['anthropic/claude-haiku-5.5', true, true],
    ['anthropic/claude-sonnet-5.5', true, false],
    ['openai/gpt-6-luna', false, false],
  ]);
  const unpinned = switchable(acct, 'anthropic/typed-by-hand', 'claude');
  assert.deepEqual(unpinned[0]?.model.id, 'anthropic/typed-by-hand', 'the session\'s own model leads even when not pinned');
  assert.equal(unpinned[0]?.ok, true);
  // A session forced onto Claude Code with a non-Anthropic model can still stay on it.
  assert.equal(switchable(acct, 'openai/gpt-6-luna', 'claude')[0]?.ok, true);
});

const acct = (id: string, name: string, o: Partial<Account> = {}): Account => ({
  id, name, kind: 'openrouter', baseUrl: '', hasKey: true, last4: '', useInOrchestrator: true, prefs: { ...DEFAULT_PREFS },
  models: [{ id: `${id}/m1`, priceIn: null, priceOut: null, tools: true }], ...o,
});

test('a helper may use a pinned model of any account that has a key and is allowed in flows', () => {
  const list = [acct('a', 'Main'), acct('b', 'Off', { useInOrchestrator: false }), acct('c', 'NoKey', { hasKey: false })];
  assert.deepEqual(helperChoices(list), [{ value: 'a|a/m1', label: 'a/m1' }], 'one usable account: its name adds nothing');
  const two = [...list, acct('d', 'Work')];
  assert.deepEqual(helperChoices(two).map((c) => c.label), ['Main · a/m1', 'Work · d/m1']);
});

test('helper values round-trip and refuse nonsense', () => {
  assert.equal(helperValue({ account: 'a', model: 'x/y:free' }), 'a|x/y:free');
  assert.deepEqual(parseHelperValue('a|x/y:free'), { account: 'a', model: 'x/y:free' });
  assert.equal(parseHelperValue(''), null);
  assert.equal(parseHelperValue('|m'), null);
  assert.equal(parseHelperValue('a|'), null);
  assert.equal(helperValue(undefined), '');
});

test('settings change one thing at a time and never in place', () => {
  const s: AccountSettings = { helpers: {}, drivers: {}, helperNames: ['suggest'] };
  const a = withHelper(s, 'suggest', 'a|m');
  assert.deepEqual(a.helpers, { suggest: { account: 'a', model: 'm' } });
  assert.deepEqual(s.helpers, {}, 'the original is untouched');
  assert.deepEqual(withHelper(a, 'suggest', '').helpers, {}, 'empty puts it back on Claude');
  const d = withDriver(s, ' ~Google/gemini ', 'claude');
  assert.deepEqual(d.drivers, { google: 'claude' });
  assert.deepEqual(withDriver(d, 'google', null).drivers, {});
  assert.equal(withDriver(s, '///', 'codex'), s, 'a vendor that cleans to nothing is ignored');
  assert.equal(cleanVendor('Open AI!'), 'openai');
});

test('the router pages are one address a line', () => {
  assert.deepEqual(intelFromText(' https://a.test/board \n\n  https://b.test\n'), ['https://a.test/board', 'https://b.test']);
  assert.deepEqual(intelFromText(''), []);
  const s: AccountSettings = { helpers: {}, drivers: {}, helperNames: [] };
  assert.deepEqual(withIntel(s, 'https://a.test').intel, ['https://a.test']);
  assert.equal(s.intel, undefined, 'the original is untouched');
});

// ---- web accounts ----

const web = (id: string, kind: Account['kind'], o: Partial<Account> = {}): Account => ({
  id,
  name: id,
  kind,
  baseUrl: '',
  hasKey: true,
  last4: '1234',
  useInOrchestrator: true,
  models: [],
  prefs: { ...DEFAULT_PREFS },
  usdPerCredit: LIST_CREDIT_USD[kind as 'firecrawl'],
  usdPerCreditSet: false,
  ...o,
});

test('Firecrawl, Tavily and Serper are the web kinds, and the others are not', () => {
  assert.deepEqual(WEB_KINDS, ['firecrawl', 'tavily', 'serper']);
  assert.ok(isWeb('tavily') && isWeb('serper') && isWeb('firecrawl'));
  assert.ok(!isWeb('openrouter') && !isWeb('openai-compatible'));
  assert.equal(KIND_LABEL.tavily, 'Tavily');
  assert.equal(KIND_LABEL.serper, 'Serper');
});

test('a price per credit is blank for the list price, or a dollar amount above nothing and at most one', () => {
  assert.equal(parseCreditPrice(''), null);
  assert.equal(parseCreditPrice('  '), null);
  assert.equal(parseCreditPrice('$0.005'), 0.005);
  assert.equal(parseCreditPrice('0.00083'), 0.00083);
  assert.equal(parseCreditPrice('1'), 1);
  for (const bad of ['0', '-1', '1.5', 'cheap', 'NaN', 'Infinity']) assert.equal(parseCreditPrice(bad), 'bad', bad);
  assert.equal(creditPriceLabel(0.008), '$0.008 per credit');
  assert.equal(creditPriceLabel(0.00083), '$0.00083 per credit');
});

test('an account keeps a price of its own, and the list price stays the list price', () => {
  assert.equal(inputOf(web('t', 'tavily')).usdPerCredit, null, 'the list price is not copied into the account');
  assert.equal(inputOf(web('t', 'tavily', { usdPerCredit: 0.005, usdPerCreditSet: true })).usdPerCredit, 0.005);
  assert.equal(emptyInput('serper').usdPerCredit, null);
});

test('the list prices match what the server assumes', () => {
  assert.deepEqual(LIST_CREDIT_USD, { firecrawl: 0.00083, tavily: 0.008, serper: 0.001 });
});

test('Tavily and Serper balances say they are counted a little late; Firecrawl is exact', () => {
  const r = { usage: 40, limit: 1000, remaining: 960, unit: 'credits' as const };
  assert.equal(webBalanceLine('firecrawl', r), '960 of 1000 credits left');
  assert.equal(webBalanceLine('tavily', r), '960 of 1000 credits left (counted a little late)');
  assert.equal(webBalanceLine('serper', { usage: null, limit: null, remaining: 2465, unit: 'credits' }), '2465 credits left (counted a little late)');
  assert.equal(webBalanceLine('tavily', { usage: null, limit: null, remaining: null, unit: 'credits' }), null);
});

test('an account moves one place at a time and stays put at the ends', () => {
  assert.deepEqual(moveInOrder(['a', 'b', 'c'], 'b', -1), ['b', 'a', 'c']);
  assert.deepEqual(moveInOrder(['a', 'b', 'c'], 'b', 1), ['a', 'c', 'b']);
  assert.deepEqual(moveInOrder(['a', 'b', 'c'], 'a', -1), ['a', 'b', 'c']);
  assert.deepEqual(moveInOrder(['a', 'b', 'c'], 'c', 1), ['a', 'b', 'c']);
  assert.deepEqual(moveInOrder(['a', 'b'], 'x', 1), ['a', 'b'], 'an account that is not listed does not move anything');
});

test('a changed order is saved whole for its tool, and a reset drops it', () => {
  const s: AccountSettings = { helpers: {}, drivers: {}, helperNames: [], webPool: { search: ['sv', 'tv', 'fc'], scrape: ['fc', 'tv'] } };
  const set = withWebOrder(s, 'search', ['tv', 'sv', 'fc']);
  assert.deepEqual(set.webOrder, { search: ['tv', 'sv', 'fc'] });
  assert.deepEqual(set.webPool?.search, ['tv', 'sv', 'fc'], 'the card shows the new order at once');
  assert.deepEqual(set.webPool?.scrape, ['fc', 'tv'], 'the other tools are left alone');
  const both = withWebOrder(set, 'scrape', ['tv', 'fc']);
  assert.deepEqual(both.webOrder, { search: ['tv', 'sv', 'fc'], scrape: ['tv', 'fc'] });
  assert.deepEqual(withoutWebOrder(both, 'search').webOrder, { scrape: ['tv', 'fc'] });
  assert.deepEqual(s.webOrder, undefined, 'the settings it was given are not changed');
});

test('the pool card lists only the tools that have accounts to put in order', () => {
  const accounts = [web('fc', 'firecrawl', { name: 'Firecrawl' }), web('tv', 'tavily', { name: 'Tavily' }), web('sv', 'serper', { name: 'Serper' })];
  const s: AccountSettings = { helpers: {}, drivers: {}, helperNames: [], webPool: { search: ['sv', 'tv', 'fc'], scrape: ['fc', 'tv'], map: ['fc', 'tv'], crawl: ['fc', 'tv'] }, webOrder: { scrape: ['fc', 'tv'] } };
  const rows = webPoolRows(s, accounts);
  assert.deepEqual(rows.map((r) => r.key), ['search', 'scrape', 'map', 'crawl']);
  assert.deepEqual(rows[0]!.accounts.map((a) => a.name), ['Serper', 'Tavily', 'Firecrawl']);
  assert.deepEqual(rows.map((r) => r.custom), [false, true, false, false], 'a tool with an order of its own is marked, so it can be reset');
  const one = webPoolRows({ ...s, webPool: { search: ['sv'] } }, accounts);
  assert.deepEqual(one, [], 'a single account has nothing to order');
  const gone = webPoolRows({ ...s, webPool: { search: ['sv', 'removed'] } }, accounts);
  assert.deepEqual(gone, [], 'an account that no longer exists is not shown');
  assert.deepEqual(webPoolRows({ helpers: {}, drivers: {}, helperNames: [] }, accounts), [], 'an older server sends no pool');
});
