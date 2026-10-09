import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ChatItem } from './chat-model.ts';
import { filesChanged, searchItems, splitPath } from './chat-insights.ts';

let n = 0;
const base = () => ({ id: `i${++n}`, seq: n, ts: null });
const edit = (path: string, old: string, neu: string, o: Partial<{ tool: string; status: 'ok' | 'error' | 'running' }> = {}): ChatItem => ({
  ...base(), kind: 'tool', tool: o.tool ?? 'Edit', summary: 'Edited', input: null, output: null, status: o.status ?? 'ok',
  diff: { path, edits: [{ old, new: neu }] },
});
const say = (text: string): ChatItem => ({ ...base(), kind: 'assistant', text });
const user = (text: string): ChatItem => ({ ...base(), kind: 'user', text, images: 0 });
const bash = (summary: string, output: string): ChatItem => ({ ...base(), kind: 'tool', tool: 'Bash', summary, input: summary, output, status: 'ok', diff: null });

test('edits to one file add up, and the newest change is listed first', () => {
  const files = filesChanged([
    edit('/p/src/a.rs', 'one\ntwo', 'one\n2\nthree'),
    edit('/p/b.md', '', 'hello\nworld', { tool: 'Write' }),
    edit('/p/src/a.rs', 'x', 'y'),
  ]);
  assert.deepEqual(files.map((f) => f.path), ['/p/src/a.rs', '/p/b.md']);
  const a = files[0]!;
  assert.equal(a.edits.length, 2);
  assert.equal(a.added, 3); // "2", "three", "y"
  assert.equal(a.removed, 2); // "two", "x"
  assert.deepEqual([a.name, a.dir], ['a.rs', 'p/src']);
});

test('a freshly written file is flagged created, a removed one deleted', () => {
  const [w, d] = [
    filesChanged([edit('/p/new.txt', '', 'hi', { tool: 'Write' })])[0]!,
    filesChanged([edit('/p/gone.txt', 'bye', '', { tool: 'Delete' })])[0]!,
  ];
  assert.equal(w.created, true);
  assert.equal(w.deleted, false);
  assert.equal(d.deleted, true);
  assert.equal(d.removed, 1);
  // Overwriting a file that was already edited is not "created".
  assert.equal(filesChanged([edit('/p/f', 'a', 'b'), edit('/p/f', '', 'c', { tool: 'Write' })])[0]!.created, false);
});

test('failed edits are counted but do not make a file changed', () => {
  assert.deepEqual(filesChanged([edit('/p/a', 'x', 'y', { status: 'error' })]), []);
  const f = filesChanged([edit('/p/a', 'x', 'y'), edit('/p/a', 'y', 'z', { status: 'error' })])[0]!;
  assert.equal(f.failed, 1);
  assert.equal(f.edits.length, 1);
});

test('an edit still running marks the file pending', () => {
  assert.equal(filesChanged([edit('/p/a', 'x', 'y', { status: 'running' })])[0]!.pending, true);
  assert.equal(filesChanged([edit('/p/a', 'x', 'y', { status: 'running' }), edit('/p/a', 'y', 'z')])[0]!.pending, false);
});

test('items with no diff and non-tool items are ignored', () => {
  assert.deepEqual(filesChanged([say('hi'), bash('ls', 'a b')]), []);
});

test('splitPath gives the name and the last two folders', () => {
  assert.deepEqual(splitPath('/a/b/c/d.ts'), { name: 'd.ts', dir: 'b/c' });
  assert.deepEqual(splitPath('d.ts'), { name: 'd.ts', dir: '' });
});

test('search finds text case-insensitively, newest first, with the words around it', () => {
  const items = [user('Please fix the Login bug'), say('Looking at the code.'), bash('grep -r login src', 'src/auth.rs: fn login()'), say('The login handler had a typo in the token check')];
  const hits = searchItems(items, 'LOGIN');
  assert.deepEqual(hits.map((h) => h.id), [items[3]!.id, items[2]!.id, items[0]!.id]);
  assert.equal(hits[0]!.match, 'login');
  assert.equal(hits[0]!.before, 'The ');
  assert.ok(hits[0]!.after.startsWith(' handler'));
  assert.deepEqual(hits.map((h) => h.kind), ['assistant', 'tool', 'user']);
});

test('search needs two characters and ignores whitespace differences', () => {
  const items = [say('one\n\n  two   three')];
  assert.deepEqual(searchItems(items, 'o'), []);
  assert.deepEqual(searchItems(items, '  '), []);
  assert.equal(searchItems(items, 'one two')[0]!.match, 'one two');
});

test('long messages are trimmed around the match with ellipses', () => {
  const long = `${'a '.repeat(60)}NEEDLE${' b'.repeat(60)}`;
  const h = searchItems([say(long)], 'needle')[0]!;
  assert.ok(h.before.startsWith('…') && h.after.endsWith('…'));
  assert.ok(h.before.length <= 37 && h.after.length <= 65);
});

test('search is capped', () => {
  const items = Array.from({ length: 100 }, () => say('match here'));
  assert.equal(searchItems(items, 'match').length, 60);
  assert.equal(searchItems(items, 'match', 5).length, 5);
});

test("a file's closing newline is not counted as an added line", () => {
  const f = filesChanged([edit('/p/new.txt', '', 'a\nb\n', { tool: 'Write' })])[0]!;
  assert.equal(f.added, 2);
  assert.equal(filesChanged([edit('/p/e.txt', 'a\n', 'a\nb\n')])[0]!.added, 1);
});
