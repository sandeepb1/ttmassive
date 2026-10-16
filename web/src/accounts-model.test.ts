import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_PREFS,
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
