import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  applySuggestion,
  attachmentToken,
  findTrigger,
  insertToken,
  removeToken,
  splitUploads,
} from './composer-text.ts';

test('a slash command is recognised only at the start of the message', () => {
  assert.deepEqual(findTrigger('/com', 4), { kind: 'slash', query: 'com', start: 0, end: 4 });
  assert.deepEqual(findTrigger('/', 1), { kind: 'slash', query: '', start: 0, end: 1 });
  assert.equal(findTrigger('hello /com', 10), null);
  assert.equal(findTrigger('/compact now', 12), null); // the command is finished
  assert.equal(findTrigger('', 0), null);
});

test('an @ starts a file word anywhere it begins a word', () => {
  assert.deepEqual(findTrigger('look at @src/ma', 15), { kind: 'file', query: 'src/ma', start: 8, end: 15 });
  assert.deepEqual(findTrigger('@x', 2), { kind: 'file', query: 'x', start: 0, end: 2 });
  assert.deepEqual(findTrigger('a\n@b', 4), { kind: 'file', query: 'b', start: 2, end: 4 });
  assert.equal(findTrigger('me@example.com', 14), null); // an email is not a file reference
  assert.equal(findTrigger('@src/main.rs and', 16), null);
});

test('the trigger follows the cursor, not the end of the text', () => {
  const text = 'fix @sr please';
  assert.deepEqual(findTrigger(text, 7), { kind: 'file', query: 'sr', start: 4, end: 7 });
  assert.equal(findTrigger(text, 14), null);
});

test('a suggestion replaces the typed word and leaves the cursor after it', () => {
  const t = findTrigger('look at @ma', 11)!;
  assert.deepEqual(applySuggestion('look at @ma', t, '@src/main.rs '), { text: 'look at @src/main.rs ', cursor: 21 });
  const s = findTrigger('/co', 3)!;
  assert.deepEqual(applySuggestion('/co', s, '/compact '), { text: '/compact ', cursor: 9 });
});

test('a suggestion does not leave a double space, and keeps the text after the cursor', () => {
  const t = findTrigger('see @ma and more', 7)!;
  assert.deepEqual(applySuggestion('see @ma and more', t, '@src/main.rs '), {
    text: 'see @src/main.rs and more',
    cursor: 17,
  });
});

test('attaching puts a token at the cursor, spaced from the word before it', () => {
  const token = attachmentToken('/data/uploads/u/1.jpg');
  assert.equal(token, '@/data/uploads/u/1.jpg ');
  assert.deepEqual(insertToken('', 0, token), { text: token, cursor: token.length });
  assert.equal(insertToken('what is this', 12, token).text, `what is this ${token}`);
  assert.equal(insertToken('what is this ', 13, token).text, `what is this ${token}`);
  assert.equal(insertToken('ab cd', 2, token).text, `ab ${token} cd`);
});

test('removing an attachment takes its token out', () => {
  const token = attachmentToken('/x/uploads/u/1.jpg');
  assert.equal(removeToken(`describe ${token}please`, token), 'describe please');
  assert.equal(removeToken('no token here', token), 'no token here');
});

test('uploaded images are pulled out of a sent message for thumbnails', () => {
  const msg = 'what is this @/Users/me/Library/ttm/uploads/7d4e/65d5-0.png ? and @/p/uploads/aa/b.jpg too';
  assert.deepEqual(splitUploads(msg), {
    text: 'what is this ? and too',
    images: ['/api/uploads/7d4e/65d5-0.png', '/api/uploads/aa/b.jpg'],
  });
  // Files that are not in an uploads folder, or not images, stay as plain text.
  assert.deepEqual(splitUploads('read @src/main.rs and @/tmp/x.png'), {
    text: 'read @src/main.rs and @/tmp/x.png',
    images: [],
  });
  assert.deepEqual(splitUploads('@/a/uploads/u/1.png'), { text: '', images: ['/api/uploads/u/1.png'] });
});

test('path-like tricks in a message cannot point the thumbnail elsewhere', () => {
  const r = splitUploads('@/a/uploads/../../etc/passwd.png');
  assert.deepEqual(r.images, []);
});
