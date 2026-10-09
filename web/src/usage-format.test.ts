import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ago,
  compact,
  countdown,
  isStale,
  level,
  stripSegments,
  tokenLine,
  type ToolUsage,
} from './usage-format.ts';

const NOW = 1_791_500_000;

const usage = (over: Partial<ToolUsage> = {}): ToolUsage => ({
  five_hour: null,
  seven_day: null,
  per_model: [],
  plan: null,
  fetched_at: NOW,
  source: 'api',
  error: null,
  ...over,
});

test('colour thresholds: amber from 75%, red from 90%', () => {
  assert.equal(level(0), 'ok');
  assert.equal(level(74.9), 'ok');
  assert.equal(level(75), 'warn');
  assert.equal(level(89.9), 'warn');
  assert.equal(level(90), 'crit');
  assert.equal(level(100), 'crit');
});

test('countdown reads naturally and never goes negative', () => {
  assert.equal(countdown(null, NOW), '');
  assert.equal(countdown(NOW - 5, NOW), 'now');
  assert.equal(countdown(NOW + 61, NOW), '2m');
  assert.equal(countdown(NOW + 3600 + 5 * 60, NOW), '1h05m');
  assert.equal(countdown(NOW + 2 * 3600 + 10 * 60, NOW), '2h10m');
  assert.equal(countdown(NOW + 2 * 86400 + 3 * 3600, NOW), '2d3h');
});

test('age of the numbers', () => {
  assert.equal(ago(0, NOW), 'never');
  assert.equal(ago(NOW - 10, NOW), 'just now');
  assert.equal(ago(NOW + 5, NOW), 'just now'); // a clock a little behind the server's
  assert.equal(ago(NOW - 180, NOW), '3m ago');
  assert.equal(ago(NOW - 2 * 3600, NOW), '2h ago');
  assert.equal(ago(NOW - 3 * 86400, NOW), '3d ago');
});

test('numbers older than half an hour, or never read, are stale', () => {
  assert.equal(isStale(usage({ fetched_at: NOW - 60 }), NOW), false);
  assert.equal(isStale(usage({ fetched_at: NOW - 31 * 60 }), NOW), true);
  assert.equal(isStale(usage({ fetched_at: 0 }), NOW), true);
});

test('token counts are compact', () => {
  assert.equal(compact(0), '0');
  assert.equal(compact(950), '950');
  assert.equal(compact(1000), '1k');
  assert.equal(compact(1234), '1.2k');
  assert.equal(compact(12_345), '12k');
  assert.equal(compact(9_999), '10k');
  assert.equal(compact(999_999), '1M');
  assert.equal(compact(1_200_000), '1.2M');
  assert.equal(compact(34_000_000), '34M');
});

test('a session line counts fresh and cache-written tokens as input, and hides empty cache', () => {
  assert.equal(
    tokenLine({ tool: 'claude', input: 10, output: 4100, cache_read: 1_200_000, cache_write: 12_000 }),
    '12k in · 4.1k out · 1.2M cached',
  );
  assert.equal(
    tokenLine({ tool: 'codex', input: 500, output: 20, cache_read: 0, cache_write: 0 }),
    '500 in · 20 out',
  );
});

test('strip segments carry a level each, and say when there is nothing to show', () => {
  const both = usage({
    five_hour: { used_pct: 98, resets_at: NOW + 60 },
    seven_day: { used_pct: 15.4, resets_at: NOW + 600 },
  });
  assert.deepEqual(stripSegments(both), {
    items: [
      { text: '5h 98%', level: 'crit' },
      { text: '7d 15%', level: 'ok' },
    ],
    unavailable: false,
  });
  // A plan with only a weekly window.
  assert.deepEqual(stripSegments(usage({ seven_day: { used_pct: 80, resets_at: null } })).items, [
    { text: '7d 80%', level: 'warn' },
  ]);
  assert.equal(stripSegments(usage({ error: 'token expired' })).unavailable, true);
});
