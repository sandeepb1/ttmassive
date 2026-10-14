import assert from 'node:assert/strict';
import { test } from 'node:test';
import { UNREADABLE_GRACE_MS, chip, chipText, doingNow, lastUserTs, observe, skewFromDate, type Tracker } from './activity-chip.ts';
import type { ChatItem, Todo } from './chat-model.ts';

const tool = (status: 'running' | 'ok', t = 'Bash', summary = 'cargo test'): ChatItem => ({
  id: t + summary, seq: 1, ts: null, kind: 'tool', tool: t, summary, input: null, output: null, status, diff: null,
});
const user = (ts: number | null): ChatItem => ({ id: 'u', seq: 1, ts, kind: 'user', text: 'x', images: 0 });
const todo = (status: Todo['status'], content: string, active: string | null = null): Todo => ({ status, content, active });

test('the to-do in progress says what the agent is doing', () => {
  assert.equal(doingNow([todo('completed', 'a'), todo('in_progress', 'Run tests', 'Running the tests'), todo('pending', 'c')], []), 'Running the tests');
  assert.equal(doingNow([todo('in_progress', 'Write docs')], []), 'Write docs'); // no active wording: the item itself
});

test('otherwise the tool it is running in this turn', () => {
  assert.equal(doingNow([], [user(1), tool('ok'), tool('running', 'Edit', 'src/a.rs')]), 'Edit src/a.rs');
  assert.equal(doingNow([], [user(1), tool('ok')]), null); // nothing running
  assert.equal(doingNow([], [tool('running'), user(1)]), null); // a tool from before your message is another turn
  assert.equal(doingNow([], []), null);
});

test('what it is doing is kept to one short line', () => {
  const d = doingNow([todo('in_progress', 'x', 'a\n   very    long\ttext '.repeat(20))], [])!;
  assert.ok(d.length <= 56 && d.endsWith('…') && !d.includes('\n'));
});

test('the latest user message time', () => {
  assert.equal(lastUserTs([user(100), tool('ok'), user(200), tool('ok')]), 200);
  assert.equal(lastUserTs([tool('ok')]), null);
  assert.equal(lastUserTs([user(null)]), null);
});

test('observing: a change starts the clock, the same reading does not restart it', () => {
  const a = observe(null, 'working', 1000);
  assert.deepEqual(a, { activity: 'working', since: 1000, sawStart: false }); // arrived mid-turn
  assert.equal(observe(a, 'working', 5000), a);
  const b = observe(a, 'idle', 6000);
  assert.deepEqual(observe(b, 'working', 9000), { activity: 'working', since: 9000, sawStart: true });
});

test('a screen that is unreadable for a moment does not reset a long turn', () => {
  const a = observe(null, 'working', 1000);
  const blip = observe(a, 'unknown', 60_000);
  assert.equal(blip.activity, 'working');
  assert.equal(blip.since, 1000, 'the clock keeps its start');
  const back = observe(blip, 'working', 62_000);
  assert.deepEqual([back.activity, back.since, back.sawStart, back.unreadableSince], ['working', 1000, false, undefined]);
  assert.equal(observe(null, 'unknown', 5).activity, 'idle');
});

test('a screen that stays unreadable is taken as nothing going on, so the chip cannot stick', () => {
  let t = observe(null, 'working', 0);
  t = observe(t, 'unknown', 5_000);
  t = observe(t, 'unknown', 5_000 + UNREADABLE_GRACE_MS - 1);
  assert.equal(t.activity, 'working', 'still within the grace');
  t = observe(t, 'unknown', 5_000 + UNREADABLE_GRACE_MS);
  assert.equal(t.activity, 'idle');
  assert.equal(chip(t, { doing: 'x', turnStart: null }, 0, 99_000), null);
});

test('a reading in between restarts the grace period', () => {
  let t = observe(null, 'waiting', 0);
  t = observe(t, 'unknown', 1_000);
  t = observe(t, 'waiting', 8_000); // readable again
  t = observe(t, 'unknown', 9_000); // a new blip begins here, not at 1 000
  t = observe(t, 'unknown', 9_000 + UNREADABLE_GRACE_MS - 1);
  assert.equal(t.activity, 'waiting');
});

test('idle shows nothing', () => {
  assert.equal(chip(null, { doing: null, turnStart: null }, 0, 1000), null);
  assert.equal(chip(observe(null, 'idle', 0), { doing: 'x', turnStart: 1 }, 0, 1000), null);
});

test('waiting counts from when it was first seen', () => {
  const t = observe(observe(null, 'idle', 0), 'waiting', 10_000);
  const c = chip(t, { doing: 'ignored', turnStart: 5 }, 0, 70_000)!;
  assert.deepEqual([c.kind, c.label, c.detail, c.secs], ['waiting', 'Waiting for you', '', 60]);
});

test('working after watching it begin uses the page clock, whatever the transcript says', () => {
  const t = observe(observe(null, 'idle', 0), 'working', 100_000);
  // The transcript still holds last turn\'s message from an hour ago.
  const c = chip(t, { doing: 'Running the tests', turnStart: 100 - 3600 }, 0, 130_000)!;
  assert.deepEqual([c.kind, c.detail, c.secs], ['working', 'Running the tests', 30]);
});

test('working found already under way uses when you sent the message', () => {
  const t: Tracker = observe(null, 'working', 1_000_000); // page just opened
  const nowMs = 1_000_000;
  const c = chip(t, { doing: null, turnStart: nowMs / 1000 - 600 }, 0, nowMs)!;
  assert.equal(c.secs, 600);
});

test('server clock skew is allowed for', () => {
  const t = observe(null, 'working', 0);
  // The server is 30 s ahead of this page, so a message "100 s ago" by the server is 100 s, not 70.
  const nowMs = 500_000;
  const c = chip(t, { doing: null, turnStart: (nowMs + 30_000) / 1000 - 100 }, 30_000, nowMs)!;
  assert.equal(c.secs, 100);
});

test('a start time that makes no sense is ignored', () => {
  const t = observe(null, 'working', 0);
  const nowMs = 100_000;
  assert.equal(chip(t, { doing: null, turnStart: nowMs / 1000 + 50 }, 0, nowMs)!.secs, 100); // in the future
  assert.equal(chip(t, { doing: null, turnStart: nowMs / 1000 - 13 * 3600 }, 0, nowMs)!.secs, 100); // yesterday
  assert.equal(chip(t, { doing: null, turnStart: null }, 0, nowMs)!.secs, 100);
});

test('the skew comes from a Date header', () => {
  assert.equal(skewFromDate('Wed, 14 Oct 2026 12:00:30 GMT', Date.parse('Wed, 14 Oct 2026 12:00:00 GMT')), 30_000);
  assert.equal(skewFromDate(null, 5), 0);
  assert.equal(skewFromDate('not a date', 5), 0);
});

test('the text reads as one line', () => {
  assert.equal(chipText({ kind: 'working', label: 'Working', detail: 'Running the tests', secs: 80 }), 'Working · Running the tests · 1m 20s');
  assert.equal(chipText({ kind: 'waiting', label: 'Waiting for you', detail: '', secs: 7 }), 'Waiting for you · 7s');
});
