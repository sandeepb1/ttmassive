import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rank, remember, score, type Item } from './command-model.ts';

const item = (id: string, label: string, extra: Partial<Item> = {}): Item => ({ id, label, run: () => {}, ...extra });
const items = [
  item('split', 'New pane', { group: 'Panes' }),
  item('close', 'Close this pane', { keywords: 'kill remove' }),
  item('search', 'Search history', { group: 'Terminal' }),
  item('settings', 'Settings'),
];

test('every word must match; a miss is -1', () => {
  assert.ok(score('new pane', 'New pane') > 0);
  assert.equal(score('new zzz', 'New pane'), -1);
  assert.equal(score('', 'anything'), 0);
});

test('a word at the start of a word beats one in the middle, and in-order letters match weakly', () => {
  assert.ok(score('pane', 'New pane') > score('ane', 'New pane'));
  assert.ok(score('ane', 'New pane') > score('nwp', 'New pane'));
  assert.ok(score('nwp', 'New pane') > 0);
  assert.equal(score('settin', 'Show another session in a new pane'), -1, 'too spread out to be a match');
});

test('typing ranks matches and drops the rest; keywords and group count', () => {
  assert.deepEqual(rank(items, 'pane', []).map((i) => i.id), ['split', 'close']);
  assert.deepEqual(rank(items, 'kill', []).map((i) => i.id), ['close']);
  assert.deepEqual(rank(items, 'terminal', []).map((i) => i.id), ['search']);
  assert.deepEqual(rank(items, 'qqq', []), []);
});

test('with nothing typed, recent commands lead in order of use and the rest keep their order', () => {
  assert.deepEqual(rank(items, '', ['settings', 'close']).map((i) => i.id), ['settings', 'close', 'split', 'search']);
  assert.deepEqual(rank(items, '  ', []).map((i) => i.id), ['split', 'close', 'search', 'settings']);
});

test('a recently used command gets a boost over an equal match', () => {
  const two = [item('a', 'Open one'), item('b', 'Open two')];
  assert.deepEqual(rank(two, 'open', ['b']).map((i) => i.id), ['b', 'a']);
});

test('remember moves an id to the front without duplicating and caps the list', () => {
  assert.deepEqual(remember(['a', 'b', 'c'], 'c'), ['c', 'a', 'b']);
  assert.deepEqual(remember(['a', 'b'], 'z', 2), ['z', 'a']);
});
