import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dayLabel, indexedAgo, jobLine, plural, preview, scoreLabel, shortPath } from './memory-model.ts';

const today = new Date(2026, 9, 14, 15, 30); // 14 Oct 2026, mid-afternoon

test('days read naturally', () => {
  assert.equal(dayLabel('2026-10-14', today), 'Today');
  assert.equal(dayLabel('2026-10-13', today), 'Yesterday');
  assert.equal(dayLabel('2026-10-09', today), 'Fri 9 Oct');
  assert.equal(dayLabel('2025-12-31', today), 'Wed 31 Dec 2025');
  assert.equal(dayLabel('not a date', today), 'not a date');
});

test('a day at 00:05 still counts as today', () => {
  assert.equal(dayLabel('2026-10-14', new Date(2026, 9, 14, 0, 5)), 'Today');
});

test('scores are percentages, kept in range', () => {
  assert.equal(scoreLabel(0.8689), '87%');
  assert.equal(scoreLabel(1.4), '100%');
  assert.equal(scoreLabel(-1), '0%');
});

test('paths are shortened to the home folder', () => {
  assert.equal(shortPath('/Users/sandeep/Documents/odev/ttmassive'), '~/Documents/odev/ttmassive');
  assert.equal(shortPath('/home/me'), '~');
  assert.equal(shortPath('/srv/apps/site/api'), '…/site/api');
  assert.equal(shortPath('/tmp'), '/tmp');
});

test('a preview drops the heading and joins the bullets', () => {
  assert.equal(preview('### 23:43\n- User asked for a tunnel.\n- Claude added it.'), 'User asked for a tunnel. · Claude added it.');
  assert.equal(preview('### only a heading'), '');
  const long = preview(`- ${'word '.repeat(100)}`, 40);
  assert.ok(long.length <= 41, 'cut to the limit');
  assert.ok(long.endsWith('word…'), 'no dangling space before the ellipsis');
});

test('job lines say what is happening or what went wrong', () => {
  const idle = { running: false, started: 0, finished: 0, error: null };
  assert.equal(jobLine(idle, 'Indexing…', 'Indexing failed'), '');
  assert.equal(jobLine({ ...idle, running: true }, 'Indexing…', 'Indexing failed'), 'Indexing…');
  assert.equal(jobLine({ ...idle, error: 'boom' }, 'Indexing…', 'Indexing failed'), 'Indexing failed: boom');
});

test('index age', () => {
  const now = Date.parse('2026-10-14T14:00:00Z');
  assert.equal(indexedAgo(null, now), 'never');
  assert.equal(indexedAgo('garbage', now), 'unknown');
  assert.equal(indexedAgo('2026-10-14T13:59:50.710205Z', now), 'just now');
  assert.equal(indexedAgo('2026-10-14T13:50:00Z', now), '10m ago');
  assert.equal(indexedAgo('2026-10-14T09:00:00Z', now), '5h ago');
  assert.equal(indexedAgo('2026-10-10T14:00:00Z', now), '4d ago');
});

test('plurals', () => {
  assert.equal(plural(1, 'entry', 'entries'), '1 entry');
  assert.equal(plural(3, 'entry', 'entries'), '3 entries');
  assert.equal(plural(0, 'day'), '0 days');
});
