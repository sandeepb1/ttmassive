import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readSessionCache, writeSessionCache } from './sessions-cache.ts';

const memory = (init: Record<string, string> = {}) => {
  const m = new Map(Object.entries(init));
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
};
const s = (id: string) => ({ id }) as never;

test('what was written is read back', () => {
  const store = memory();
  writeSessionCache([s('a'), s('b')], store);
  assert.deepEqual(readSessionCache(store)?.map((x) => x.id), ['a', 'b']);
});

test('nothing cached is null', () => {
  assert.equal(readSessionCache(memory()), null);
});

test('an empty list is not worth showing', () => {
  const store = memory();
  writeSessionCache([], store);
  assert.equal(readSessionCache(store), null);
});

test('garbage is ignored rather than thrown', () => {
  assert.equal(readSessionCache(memory({ 'ttm.sessions.cache': '{not json' })), null);
  assert.equal(readSessionCache(memory({ 'ttm.sessions.cache': '{"id":"a"}' })), null);
  assert.equal(readSessionCache(memory({ 'ttm.sessions.cache': '[{"name":"a"}]' })), null);
});

test('a store that throws does not break the app', () => {
  const broken = { getItem: () => { throw new Error('blocked'); }, setItem: () => { throw new Error('full'); } };
  assert.doesNotThrow(() => writeSessionCache([s('a')], broken));
  assert.equal(readSessionCache(broken), null);
  assert.equal(readSessionCache(null), null);
});
