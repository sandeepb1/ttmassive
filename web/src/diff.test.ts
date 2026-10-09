import { test } from 'node:test';
import assert from 'node:assert/strict';
import { lineDiff, trimContext } from './diff.ts';

const sym = (d: ReturnType<typeof lineDiff>) => d.map((l) => `${l.t === 'add' ? '+' : l.t === 'del' ? '-' : ' '}${l.v}`);

test('a replaced line shows as removed then added, with shared lines as context', () => {
  assert.deepEqual(sym(lineDiff('a\nb\nc', 'a\nB\nc')), [' a', '-b', '+B', ' c']);
});

test('inserted and deleted lines in the middle', () => {
  assert.deepEqual(sym(lineDiff('a\nc', 'a\nb\nc')), [' a', '+b', ' c']);
  assert.deepEqual(sym(lineDiff('a\nb\nc', 'a\nc')), [' a', '-b', ' c']);
});

test('a new file is all additions and an emptied one all deletions', () => {
  assert.deepEqual(sym(lineDiff('', 'x\ny')), ['+x', '+y']);
  assert.deepEqual(sym(lineDiff('x\ny', '')), ['-x', '-y']);
  assert.deepEqual(lineDiff('', ''), []);
});

test('identical text is all context', () => {
  assert.deepEqual(sym(lineDiff('a\nb', 'a\nb')), [' a', ' b']);
});

test('moved and repeated lines still produce a valid diff', () => {
  const oldText = 'a\nb\nb\nc';
  const newText = 'b\nc\na\nb';
  const d = lineDiff(oldText, newText);
  // Whatever the exact alignment, replaying it must rebuild both sides.
  assert.equal(d.filter((l) => l.t !== 'add').map((l) => l.v).join('\n'), oldText);
  assert.equal(d.filter((l) => l.t !== 'del').map((l) => l.v).join('\n'), newText);
});

test('huge edits fall back to remove-all then add-all instead of building a giant table', () => {
  const a = Array.from({ length: 1000 }, (_, i) => `old${i}`).join('\n');
  const b = Array.from({ length: 1000 }, (_, i) => `new${i}`).join('\n');
  const d = lineDiff(a, b);
  assert.equal(d.length, 2000);
  assert.equal(d.filter((l) => l.t === 'del').length, 1000);
});

test('context trimming keeps lines near a change and counts what it hides', () => {
  const text = Array.from({ length: 20 }, (_, i) => `l${i}`);
  const changed = [...text];
  changed[10] = 'CHANGED';
  const trimmed = trimContext(lineDiff(text.join('\n'), changed.join('\n')), 2);
  assert.deepEqual(
    trimmed.map((l) => (l.t === 'gap' ? `gap${l.n}` : `${l.t}:${l.v}`)),
    ['gap8', 'ctx:l8', 'ctx:l9', 'del:l10', 'add:CHANGED', 'ctx:l11', 'ctx:l12', 'gap7'],
  );
});
