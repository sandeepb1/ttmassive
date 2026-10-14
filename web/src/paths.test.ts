import assert from 'node:assert/strict';
import { test } from 'node:test';
import { findPaths } from './paths.ts';

const paths = (s: string) => findPaths(s).map((m) => m.path);
const one = (s: string) => {
  const f = findPaths(s);
  assert.equal(f.length, 1, `${s} -> ${JSON.stringify(f)}`);
  return f[0]!;
};

test('a path with a line and column', () => {
  const m = one('  --> src/main.rs:42:7');
  assert.deepEqual([m.path, m.line, m.col], ['src/main.rs', 42, 7]);
  assert.equal('  --> src/main.rs:42:7'.slice(m.start, m.end), 'src/main.rs:42:7');
});

test('a path with only a line, or none', () => {
  assert.deepEqual([one('src/a.ts:9').line, one('src/a.ts:9').col], [9, null]);
  const m = one('edited web/src/pane.ts today');
  assert.deepEqual([m.path, m.line], ['web/src/pane.ts', null]);
  assert.equal('edited web/src/pane.ts today'.slice(m.start, m.end), 'web/src/pane.ts');
});

test('parenthesised locations, as TypeScript prints them', () => {
  const m = one('src/app.ts(12,5): error TS2322');
  assert.deepEqual([m.path, m.line, m.col], ['src/app.ts', 12, 5]);
  assert.equal(one('a/b.ts(3)').line, 3);
});

test('explicit locations need no extension', () => {
  assert.deepEqual(paths('see ./bin/run and ../x/y and ~/notes and /usr/bin/env'), ['./bin/run', '../x/y', '~/notes', '/usr/bin/env']);
});

test('known extensionless names and dotfiles', () => {
  assert.deepEqual(paths('edit docs/Makefile and .gitignore:3 and app/.env'), ['docs/Makefile', '.gitignore', 'app/.env']);
  assert.equal(one('Makefile:12').line, 12);
});

test('a bare file name needs a code extension', () => {
  assert.deepEqual(paths('open main.rs or index.html please'), ['main.rs', 'index.html']);
  assert.deepEqual(paths('visit example.com, e.g. v1.2.3, 3.14'), []);
  // Known ambiguity: `node.js` is also a product name. A stray underline on hover costs less than a missed file.
  assert.deepEqual(paths('node.js'), ['node.js']);
  assert.deepEqual(paths('word.rs:7'), ['word.rs']);
});

test('slash commands, prose and numbers are not files', () => {
  assert.deepEqual(paths('/model and /clear then /status'), []);
  assert.deepEqual(paths('yes/no and and/or 1/2 12:30:45 100%'), []);
  assert.deepEqual(paths('-- --flag --config=a'), []);
});

test('URLs are left to the link handler', () => {
  assert.deepEqual(paths('see https://example.com/docs/readme.md and http://localhost:3000/a.js'), []);
  assert.deepEqual(paths('ftp://x.org/file.txt'), []);
});

test('trailing punctuation is not part of the name', () => {
  assert.deepEqual(paths('(see src/a.rs). Then src/b.rs, and src/c.rs.'), ['src/a.rs', 'src/b.rs', 'src/c.rs']);
  const t = 'failed at src/a.rs:10.';
  const m = one(t);
  assert.equal(t.slice(m.start, m.end), 'src/a.rs:10');
});

test('several paths in a line keep their own offsets', () => {
  const t = 'a/x.rs:1 b/y.rs:2:3';
  const [p, q] = findPaths(t);
  assert.equal(t.slice(p!.start, p!.end), 'a/x.rs:1');
  assert.equal(t.slice(q!.start, q!.end), 'b/y.rs:2:3');
});

test('scoped packages and nested folders', () => {
  assert.deepEqual(paths('node_modules/@types/node/index.d.ts'), ['node_modules/@types/node/index.d.ts']);
});

test('nothing in empty or plain text', () => {
  assert.deepEqual(findPaths(''), []);
  assert.deepEqual(findPaths('The quick brown fox'), []);
});
