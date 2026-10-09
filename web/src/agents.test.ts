import { test } from 'node:test';
import assert from 'node:assert/strict';
import { agentForItem, asTree, childrenOf, elapsedSecs, fmtElapsed, isAgentTool, stateOf, workLine, type SubAgent } from './agents.ts';

const ag = (id: string, o: Partial<SubAgent> = {}): SubAgent => ({
  id, kind: 'Explore', description: id, tool_use_id: `t-${id}`, depth: 1, parent: null, running: false, started: 100, last: 160,
  tools: 3, last_action: 'Read x', result: null, has_log: true, ...o,
});

test('a tool call finds the helper it started, by the tool call id', () => {
  const list = [ag('a'), ag('b')];
  assert.equal(agentForItem(list, 't-b')?.id, 'b');
  assert.equal(agentForItem(list, 'nope'), undefined);
});

test('only Agent and Task tool calls start helpers', () => {
  assert.ok(isAgentTool('Agent') && isAgentTool('Task'));
  assert.ok(!isAgentTool('Bash') && !isAgentTool('Skill'));
});

test('helpers lay out as a tree, children after their parent', () => {
  const list = [ag('a'), ag('c', { parent: 'b' }), ag('b', { parent: 'a' }), ag('d')];
  assert.deepEqual(asTree(list).map((n) => `${n.level}:${n.agent.id}`), ['0:a', '1:b', '2:c', '0:d']);
  assert.deepEqual(childrenOf(list, 'a').map((x) => x.id), ['b']);
  assert.deepEqual(childrenOf(list, null).map((x) => x.id), ['a', 'd']);
});

test('a helper whose parent is missing still shows, and a cycle cannot loop', () => {
  assert.deepEqual(asTree([ag('x', { parent: 'gone' })]).map((n) => n.agent.id), ['x']);
  assert.equal(asTree([ag('a', { parent: 'b' }), ag('b', { parent: 'a' })]).length, 2);
});

test('state: starting until it has a log, running while it is written, done after', () => {
  assert.equal(stateOf(ag('a', { running: true, has_log: false })), 'starting');
  assert.equal(stateOf(ag('a', { running: true })), 'running');
  assert.equal(stateOf(ag('a', { running: false })), 'done');
});

test('elapsed counts to now while running and to its last line once done', () => {
  assert.equal(elapsedSecs(ag('a', { running: true }), 130), 30);
  assert.equal(elapsedSecs(ag('a', { running: false }), 9999), 60);
  assert.equal(elapsedSecs(ag('a', { started: null }), 5), null);
  assert.equal(elapsedSecs(ag('a', { running: true, started: 200 }), 100), 0, 'never negative');
});

test('durations read as seconds, minutes or hours', () => {
  assert.deepEqual([0, 59, 60, 65, 3599, 3600, 7385].map(fmtElapsed), ['0s', '59s', '1m 00s', '1m 05s', '59m 59s', '1h 00m', '2h 03m']);
});

test('the work line pluralises and leaves out a missing last action', () => {
  assert.equal(workLine(ag('a', { tools: 1 })), '1 tool call · Read x');
  assert.equal(workLine(ag('a', { tools: 0, last_action: null })), '0 tool calls');
});
