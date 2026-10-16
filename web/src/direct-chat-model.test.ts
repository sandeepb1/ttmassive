import { test } from 'node:test';
import assert from 'node:assert/strict';
import { KEEP, chatKey, parseSaved, push, spent, spentLine, toWire, type ChatMsg } from './direct-chat-model.ts';

const u = (c: string): ChatMsg => ({ role: 'user', content: c });
const a = (c: string, costUsd?: number | null): ChatMsg => ({ role: 'assistant', content: c, costUsd });

test('only roles and text go to the server', () => {
  assert.deepEqual(toWire([u('hi'), { ...a('hello', 0.01), tools: ['scrape'], tokens: { in: 1, out: 2 } }]), [{ role: 'user', content: 'hi' }, { role: 'assistant', content: 'hello' }]);
});

test('a long conversation drops its oldest messages in pairs and still starts with the person', () => {
  let m: ChatMsg[] = [];
  for (let i = 0; i < KEEP; i++) m = push(push(m, u(`q${i}`)), a(`a${i}`));
  assert.ok(m.length <= KEEP);
  assert.equal(m[0]!.role, 'user');
  assert.equal(m.at(-1)!.content, `a${KEEP - 1}`);
  assert.deepEqual(push([u('x')], a('y')).map((x) => x.content), ['x', 'y']);
});

test('what it has cost, and the replies that could not be priced', () => {
  assert.deepEqual(spent([u('q'), a('r', 0.002), u('q2'), a('r2', 0.0005), u('q3'), a('r3', null), a('r4')]), { usd: 0.0025, unknown: 2 });
  assert.equal(spentLine([u('q'), a('r', 0.0021)]), '$0.0021 so far');
  assert.equal(spentLine([u('q'), a('r', 0.5), u('q'), a('r', 0.25)]), '$0.75 so far');
  assert.equal(spentLine([u('q'), a('r', null)]), '$0.00 so far (1 reply not priced)');
  assert.equal(spentLine([]), '$0.00 so far');
});

test('a saved conversation that is damaged comes back empty, and one that is fine comes back whole', () => {
  assert.deepEqual(parseSaved(null), []);
  assert.deepEqual(parseSaved('not json'), []);
  assert.deepEqual(parseSaved('{"a":1}'), []);
  assert.deepEqual(parseSaved(JSON.stringify([u('hi'), { role: 'system', content: 'x' }, { role: 'user' }, a('yo', 0.1)])), [u('hi'), a('yo', 0.1)], 'anything that is not a message is dropped');
  assert.equal(chatKey('acct', 'a/b:free'), 'ttm.directchat.acct.a/b:free');
});
