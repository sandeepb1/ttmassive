import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pathsToPaste, shellQuote } from './term-drop.ts';

test('plain paths are left alone, awkward ones are single-quoted', () => {
  assert.equal(shellQuote('/Users/me/.local/share/ttm/uploads/a/1-2.png'), '/Users/me/.local/share/ttm/uploads/a/1-2.png');
  assert.equal(shellQuote('/Users/me/Library/Application Support/ttm/x.png'), "'/Users/me/Library/Application Support/ttm/x.png'");
  assert.equal(shellQuote("/tmp/it's.png"), "'/tmp/it'\\''s.png'");
  assert.equal(shellQuote('/tmp/$(rm).png'), "'/tmp/$(rm).png'");
});

test('several paths are space-separated with a trailing space and no newline', () => {
  assert.equal(pathsToPaste(['/a/b.png', '/c d/e.txt']), "/a/b.png '/c d/e.txt' ");
});
