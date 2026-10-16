import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  dollars,
  orLines,
  orStrip,
  orUsedPct,
  type OrUsage,
  ago,
  compact,
  countdown,
  isStale,
  level,
  SOURCE_LABEL,
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

test('every source the server can send has a label', () => {
  // The server's `Source` enum, as it arrives in JSON. A missing label shows as "undefined".
  for (const source of ['api', 'hook', 'session_log', 'team_claude'] as const) {
    assert.ok(SOURCE_LABEL[source], `${source} has a label`);
  }
  assert.equal(SOURCE_LABEL.team_claude, 'Team Claude pool');
  assert.deepEqual(stripSegments(usage({ source: 'team_claude', five_hour: { used_pct: 97, resets_at: null } })).items.map((i) => i.text), ['5h 97%']);
});

const or = (o: Partial<OrUsage> = {}): OrUsage => ({ id: 'a', name: 'Main', spent: 12.5, daily: 0.4, monthly: 7, limit: null, remaining: null, error: null, fetchedAt: 100, ...o });

test('dollars read naturally from tenths of a cent to thousands', () => {
  assert.equal(dollars(0), '$0');
  assert.equal(dollars(0.0042), '$0.004');
  assert.equal(dollars(0.5), '$0.50');
  assert.equal(dollars(12.345), '$12.35');
  assert.equal(dollars(250.4), '$250');
  assert.equal(dollars(1234.5), '$1235');
});

test('a key with a limit shows what is left, coloured by how much is used; one without shows today or the total', () => {
  const limited = or({ limit: 50, remaining: 37.5 });
  assert.equal(orUsedPct(limited), 25);
  assert.deepEqual(orStrip(limited), { text: '$37.50 left', level: 'ok' });
  assert.equal(orStrip(or({ limit: 50, remaining: 4 })).level, 'crit');
  assert.equal(orUsedPct(or()), null, 'no limit, no percentage');
  assert.deepEqual(orStrip(or()), { text: '$0.40 today', level: 'ok' });
  assert.deepEqual(orStrip(or({ daily: null })), { text: '$12.50 spent', level: 'ok' });
  assert.deepEqual(orStrip(or({ daily: null, spent: null })), { text: '–', level: 'ok' });
  assert.deepEqual(orStrip(or({ error: 'OpenRouter answered 401' })), { text: '⚠', level: 'warn' });
  assert.equal(orUsedPct(or({ limit: 10, remaining: 12 })), 0, 'never below zero');
});

test('the panel lists what is known and skips what is not', () => {
  assert.deepEqual(orLines(or({ limit: 50, remaining: 37.5 })), ['$37.50 left of $50.00', '$0.40 today', '$7.00 this month', '$12.50 in all']);
  assert.deepEqual(orLines(or({ daily: null, monthly: null })), ['$12.50 in all']);
  assert.deepEqual(orLines(or({ spent: null, daily: null, monthly: null })), []);
});
