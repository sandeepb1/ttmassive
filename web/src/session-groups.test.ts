import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dotState, groupSessions, NO_FOLDER_KEY, PINNED_KEY, railLabel, type Groupable } from './session-groups.ts';

const s = (id: string, o: Partial<Groupable> = {}): Groupable => ({
  id, pinned: false, cwd: null, status: 'running', idle_secs: 60, activity: null, ...o,
});

test('pinned sessions come first, then one section per folder in order of appearance', () => {
  const secs = groupSessions([
    s('a', { cwd: '/w/app' }),
    s('b', { cwd: '/w/docs', pinned: true }),
    s('c', { cwd: '/w/api' }),
    s('d', { cwd: '/w/app' }),
  ]);
  assert.deepEqual(secs.map((x) => x.key), [PINNED_KEY, '/w/app', '/w/api']);
  assert.deepEqual(secs.map((x) => x.items.map((i) => i.id)), [['b'], ['a', 'd'], ['c']]);
  assert.deepEqual([secs[1]!.title, secs[1]!.sub], ['app', 'w']);
});

test('a pinned session is not repeated under its folder', () => {
  const secs = groupSessions([s('a', { cwd: '/w/app', pinned: true })]);
  assert.deepEqual(secs.map((x) => x.key), [PINNED_KEY]);
});

test('sessions without a folder share one section', () => {
  const secs = groupSessions([s('a'), s('b')]);
  assert.equal(secs.length, 1);
  assert.equal(secs[0]!.key, NO_FOLDER_KEY);
  assert.equal(secs[0]!.title, 'No folder');
  assert.equal(secs[0]!.items.length, 2);
});

test('two folders with the same name are told apart by their parent', () => {
  const secs = groupSessions([s('a', { cwd: '/one/app' }), s('b', { cwd: '/two/app' })]);
  assert.deepEqual(secs.map((x) => `${x.sub}/${x.title}`), ['one/app', 'two/app']);
});

test('no sessions, no sections', () => {
  assert.deepEqual(groupSessions([]), []);
});

test('the dot says what the agent is doing', () => {
  assert.equal(dotState(s('a', { activity: 'waiting' })), 'waiting');
  assert.equal(dotState(s('a', { activity: 'working' })), 'working');
  assert.equal(dotState(s('a', { activity: 'idle' })), 'ready');
  assert.equal(dotState(s('a', { status: 'stopped', activity: 'waiting' })), 'stopped');
  assert.equal(dotState(s('a', { status: 'exited' })), 'exited');
});

test('without screen information the dot falls back to recent output', () => {
  assert.equal(dotState(s('a', { idle_secs: 2 })), 'working');
  assert.equal(dotState(s('a', { idle_secs: 60 })), 'idle');
  assert.equal(dotState(s('a', { idle_secs: 60, activity: 'unknown' })), 'idle');
});

test('rail labels are two characters: initials of a hyphenated name, else the first two letters', () => {
  assert.equal(railLabel('claude-cedarwing'), 'CC');
  assert.equal(railLabel('api_server'), 'AS');
  assert.equal(railLabel('docs'), 'Do');
  assert.equal(railLabel('x'), 'X');
  assert.equal(railLabel('-odd-'), 'Od');
});
