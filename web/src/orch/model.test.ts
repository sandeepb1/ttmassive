import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  addEdge, answersFor, cacheHit, connectionProblem, defaultModel, emptyFlow, flowProblem, fmtTokens, freeId, gauges, layers,
  builderBranches, newNode, pauseInfo, removeNode, runOrder, runTokens, statusLabel, tidy,
} from './model.ts';
import type { Flow, Models, Role, Run, UsageSnapshot } from './types.ts';

const models: Models = { claude: ['opus', 'sonnet', 'haiku'], codex: ['gpt-6.1-sol', 'gpt-6-luna'] };
const at = { x: 0, y: 0 };

function line(): Flow {
  let f = emptyFlow('f', 'Line');
  for (const r of ['orchestrator', 'researcher', 'builder', 'judge'] as Role[]) f = { ...f, nodes: [...f.nodes, newNode(f, r, models, at)] };
  const [a, b, c, d] = f.nodes.map((n) => n.id) as [string, string, string, string];
  f = addEdge(f, a, b, 'forward');
  f = addEdge(f, b, c, 'forward');
  f = addEdge(f, c, d, 'forward');
  return f;
}

test('ids are the first free one', () => {
  assert.equal(freeId([], 'n'), 'n1');
  assert.equal(freeId(['n1', 'n3'], 'n'), 'n2');
});

test('roles get sensible default models, falling back when Codex has none', () => {
  assert.deepEqual(defaultModel('judge', models), { engine: 'claude', model: 'opus' });
  assert.deepEqual(defaultModel('builder', models), { engine: 'codex', model: 'gpt-6.1-sol' });
  assert.deepEqual(defaultModel('search', models), { engine: 'codex', model: 'gpt-6-luna' });
  const none: Models = { claude: ['opus'], codex: [] };
  assert.deepEqual(defaultModel('builder', none), { engine: 'claude', model: 'sonnet' });
  assert.deepEqual(defaultModel('search', none), { engine: 'claude', model: 'haiku' });
});

test('a new builder waits for approval and a second node of a role is numbered', () => {
  const f = line();
  assert.equal(f.nodes[2]!.checkpoint, true);
  assert.equal(f.nodes[1]!.checkpoint, false);
  assert.equal(newNode(f, 'builder', models, at).name, 'Builder 2');
  assert.equal(newNode(f, 'search', models, at).name, 'Search');
});

test('removing an agent removes its edges', () => {
  const f = removeNode(line(), 'n2');
  assert.equal(f.nodes.length, 3);
  assert.deepEqual(f.edges.map((e) => `${e.from}>${e.to}`), ['n3>n4']);
});

test('connections: no self, no duplicates, no loops', () => {
  const f = line();
  assert.match(connectionProblem(f, 'n1', 'n1', 'forward')!, /itself/);
  assert.match(connectionProblem(f, 'n1', 'n2', 'forward')!, /Already/);
  assert.match(connectionProblem(f, 'n4', 'n1', 'forward')!, /loop/);
  assert.equal(connectionProblem(f, 'n1', 'n3', 'forward'), null);
  assert.match(connectionProblem(f, 'n1', 'ghost', 'forward')!, /Both ends/);
});

test('retry edges: judge to an earlier builder only', () => {
  const f = line();
  assert.equal(connectionProblem(f, 'n4', 'n3', 'retry'), null);
  assert.match(connectionProblem(f, 'n3', 'n3', 'retry')!, /itself/);
  assert.match(connectionProblem(f, 'n2', 'n3', 'retry')!, /Only a judge/);
  assert.match(connectionProblem(f, 'n4', 'n2', 'retry')!, /builder/);
  // A builder that runs after the judge cannot be sent back to.
  let g = f;
  g = { ...g, nodes: [...g.nodes, newNode(g, 'builder', models, at)] };
  g = addEdge(g, 'n4', 'n5', 'forward');
  assert.match(connectionProblem(g, 'n4', 'n5', 'retry')!, /before/);
});

test('layers and tidy lay a flow out left to right', () => {
  const f = line();
  assert.deepEqual(layers(f), [['n1'], ['n2'], ['n3'], ['n4']]);
  const t = tidy(f);
  assert.deepEqual(t.nodes.map((n) => n.x), [0, 320, 640, 960]);
  assert.deepEqual(runOrder(f).map((n) => n.id), ['n1', 'n2', 'n3', 'n4']);
});

test('a retry edge does not change the layout, and a loop gives no layout', () => {
  const f = addEdge(line(), 'n4', 'n3', 'retry');
  assert.equal(layers(f).length, 4);
  const loop = addEdge(line(), 'n4', 'n1', 'forward');
  assert.deepEqual(layers(loop), []);
  assert.equal(tidy(loop), loop);
});

test('branches share a column', () => {
  let f = line();
  f = { ...f, nodes: [...f.nodes, newNode(f, 'search', models, at)] };
  f = addEdge(f, 'n1', 'n5', 'forward');
  f = addEdge(f, 'n5', 'n3', 'forward');
  assert.deepEqual(layers(f)[1], ['n2', 'n5']);
  const t = tidy(f);
  assert.deepEqual([t.nodes[1]!.y, t.nodes[4]!.y], [0, 190]);
});

test('flowProblem says what stops a flow being saved', () => {
  assert.equal(flowProblem(line()), null);
  assert.match(flowProblem(emptyFlow('f', ''))!, /name/);
  assert.match(flowProblem(emptyFlow('f', 'x'))!, /at least one/);
  assert.match(flowProblem(addEdge(line(), 'n4', 'n1', 'forward'))!, /loop/);
  assert.match(flowProblem(addEdge(line(), 'n2', 'n3', 'retry'))!, /judge/);
  const bad = line();
  bad.budget.claudePct = 0;
  assert.match(flowProblem(bad)!, /Budget/);
  const noModel = line();
  noModel.nodes[0]!.model = ' ';
  assert.match(flowProblem(noModel)!, /model/);
});

test('status labels say who needs to act', () => {
  assert.deepEqual(statusLabel({ state: 'running' }), { text: 'Running', tone: 'live' });
  assert.equal(statusLabel({ state: 'paused', pause: { kind: 'approval', node: 'n3' } }).tone, 'warn');
  assert.equal(statusLabel({ state: 'paused', pause: { kind: 'failed', node: 'n3', message: 'x' } }).tone, 'bad');
  assert.equal(statusLabel({ state: 'done' }).tone, 'ok');
});

function runOf(flow: Flow, status: Run['status']): Run {
  return {
    id: 'r1', flow, goal: 'g', brief: 'b', project: '/p', grill: null, baseline: { claude: 30, codex: 0 },
    overhead: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4 }, status, nodes: {}, approved: [], feedback: {}, retriesUsed: 1, created: 0,
  };
}

test('a pause says what it is waiting for and offers the right button', () => {
  const f = line();
  const a = pauseInfo(runOf(f, { state: 'paused', pause: { kind: 'approval', node: 'n3' } }))!;
  assert.match(a.title, /Builder is ready/);
  assert.match(a.detail, /change files/);
  assert.deepEqual(a.primary, { label: 'Approve Builder', action: 'approve', node: 'n3' });

  const r = pauseInfo(runOf(f, { state: 'paused', pause: { kind: 'retriesExhausted', node: 'n4' } }))!;
  assert.equal(r.primary!.action, 'resume');
  assert.match(r.primary!.label, /Accept/);
  assert.match(r.detail, /1 retry\b/);

  const b = pauseInfo(runOf(f, { state: 'paused', pause: { kind: 'budget', message: 'Claude is at 86%' } }))!;
  assert.equal(b.detail, 'Claude is at 86%');
  assert.equal(b.primary!.action, 'resume');

  const x = pauseInfo(runOf(f, { state: 'paused', pause: { kind: 'failed', node: '', message: 'ttm restarted' } }))!;
  assert.equal(x.title, 'The run was interrupted');
  assert.equal(pauseInfo(runOf(f, { state: 'running' })), null);
});

test('tokens sum across nodes and overhead, and cache hits are a share of input', () => {
  const r = runOf(line(), { state: 'running' });
  r.nodes = { n1: { status: 'done', attempts: 1, output: null, error: null, transcript: [], tokens: { input: 10, output: 20, cacheRead: 30, cacheWrite: 40 } } };
  assert.deepEqual(runTokens(r), { input: 11, output: 22, cacheRead: 33, cacheWrite: 44 });
  assert.equal(cacheHit({ input: 100, output: 9, cacheRead: 800, cacheWrite: 100 }), 0.8);
  assert.equal(cacheHit({ input: 0, output: 9, cacheRead: 0, cacheWrite: 0 }), null);
});

test('token counts are short', () => {
  assert.deepEqual([0, 999, 1000, 12_345, 999_999, 1_500_000].map(fmtTokens), ['0', '999', '1.0k', '12k', '1000k', '1.5M']);
});

test('gauges show the run’s share of each window and turn red at the budget', () => {
  const f = line(); // claude opus + sonnet, plus a codex builder
  const usage = (c: number, x: number): UsageSnapshot => ({
    claude: { five_hour: { used_pct: c, resets_at: null }, seven_day: null, error: null },
    codex: { five_hour: { used_pct: x, resets_at: null }, seven_day: null, error: null },
  });
  const run = runOf(f, { state: 'running' }); // baseline claude 30, codex 0
  const g = gauges(f, run, usage(35, 2));
  assert.deepEqual(g.map((x) => [x.engine, x.share, x.tone]), [['claude', 5, 'ok'], ['codex', 2, 'ok']]);
  assert.equal(gauges(f, run, usage(43, 2))[0]!.tone, 'warn'); // 13 of 15
  assert.equal(gauges(f, run, usage(45, 2))[0]!.tone, 'bad'); // 15 of 15
  assert.equal(gauges(f, run, usage(86, 2))[0]!.tone, 'bad'); // past the stop line
  // Started high, used little, but the window itself is close to the stop line.
  const high = { ...run, baseline: { claude: 70, codex: 0 } };
  assert.equal(gauges(f, high, usage(76, 2))[0]!.tone, 'warn');
  assert.equal(gauges(f, high, usage(60, 2))[0]!.tone, 'ok');
  // No run yet: only the window itself is known.
  assert.equal(gauges(f, null, usage(35, 2))[0]!.share, null);
  assert.equal(gauges(f, null, null)[0]!.used, null);
  // Only engines the flow uses are shown.
  const claudeOnly = { ...f, nodes: f.nodes.filter((n) => n.engine === 'claude') };
  assert.deepEqual(gauges(claudeOnly, null, null).map((x) => x.engine), ['claude']);
});

test('blank answers take the recommended one', () => {
  const pending = [{ recommended: 'SQLite' }, { recommended: 'yes' }];
  assert.deepEqual(answersFor(pending, { 1: ' no ' }), ['SQLite', 'no']);
});

test('a finished run says which branch holds each builder’s work', () => {
  const r = runOf(line(), { state: 'done' });
  assert.deepEqual(builderBranches(r), []);
  r.nodes = { n3: { status: 'done', attempts: 2, output: null, error: null, transcript: [], tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } } };
  assert.deepEqual(builderBranches(r), [{ node: 'n3', name: 'Builder', branch: 'ttm-orch/r1-n3' }]);
});
