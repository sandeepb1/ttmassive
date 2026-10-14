import assert from 'node:assert/strict';
import { test } from 'node:test';
import { looksLikePrompt, markedRows, markedTurns } from './turn-marks.ts';

test('prompt marks are told from prose', () => {
  for (const lead of ['', '> ', '❯ ', '│ > ', '  › ']) assert.ok(looksLikePrompt(lead), JSON.stringify(lead));
  for (const lead of ['I will ', 'you said: ', '> > > > > ', 'a ']) assert.ok(!looksLikePrompt(lead), JSON.stringify(lead));
});

test('rows that are your message are marked', () => {
  const rows = ['> fix the parser', '  working on it', 'I will fix the parser now', '> run tests', ''];
  assert.deepEqual(markedRows(rows, ['fix the parser', 'run tests']), [0, 3]);
});

test('the agent quoting you is not marked', () => {
  assert.deepEqual(markedRows(['the user asked: fix the parser'], ['fix the parser']), []);
});

test('case follows the same smart-case rule as the search', () => {
  assert.deepEqual(markedRows(['> FIX THE PARSER'], ['fix the parser']), [0]); // no capital: ignores case
  assert.deepEqual(markedRows(['> fix the parser'], ['Fix the parser']), []); // a capital: exact
  assert.deepEqual(markedRows(['> Fix the parser'], ['Fix the parser']), [0]);
});

test('a decorated prompt box still counts', () => {
  assert.deepEqual(markedRows(['│ > deploy it'], ['deploy it']), [0]);
  assert.deepEqual(markedRows(['╭─ > deploy it'], ['deploy it']), [0]);
});

test('empty rows and no snippets mark nothing', () => {
  assert.deepEqual(markedRows(['', '   '], ['x y z']), []);
  assert.deepEqual(markedRows(['> hello there'], []), []);
});

test('repeated snippets are fine', () => {
  assert.deepEqual(markedRows(['> go on', '> go on'], ['go on', 'go on']), [0, 1]);
});

test('each marked row knows which message it is', () => {
  const rows = ['> fix the parser', 'ok', '> run tests'];
  const turns = [{ id: 'm2', snippet: 'run tests' }, { id: 'm1', snippet: 'fix the parser' }];
  assert.deepEqual(markedTurns(rows, turns), [{ row: 0, id: 'm1' }, { row: 2, id: 'm2' }]);
});

test('a text sent twice: the lowest row is the newest of them', () => {
  const rows = ['> yes', 'x', '> yes', 'y'];
  const turns = [{ id: 'new', snippet: 'yes' }, { id: 'old', snippet: 'yes' }];
  assert.deepEqual(markedTurns(rows, turns), [{ row: 0, id: 'old' }, { row: 2, id: 'new' }]);
});
