import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  awaitingApproval, cacheBreakCount, fmtDuration, modelShown, summary,
  addEdge, answerFor, answersFor, cacheHit, choose, hasChoices, startingAnswer, writeInstead, connectionProblem, defaultModel, emptyFlow, flowProblem, fmtTokens, freeId, gauges, layers,
  builderBranches, consistent, decides, getCatalog, newCustomNode, newNode, pauseInfo, removeNode, roleDef, roleIconName, roleLabel, runOrder, runTokens,
  setCatalog, STAGES, statusLabel, tidy, writes,
} from './model.ts';
import type { Flow, Models, Role, Run, UsageSnapshot } from './types.ts';

const models: Models = { claude: ['opus', 'sonnet', 'haiku'], codex: ['gpt-6.1-sol', 'gpt-6-luna'] };
const at = { x: 0, y: 0 };
const def = (id: Role) => roleDef(id)!;

function line(): Flow {
  let f = emptyFlow('f', 'Line');
  for (const r of ['orchestrator', 'researcher', 'builder', 'judge'] as Role[]) f = { ...f, nodes: [...f.nodes, newNode(f, def(r), models, at)] };
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
  assert.deepEqual(defaultModel(def('judge'), models), { engine: 'claude', model: 'opus' });
  assert.deepEqual(defaultModel(def('builder'), models), { engine: 'codex', model: 'gpt-6.1-sol' });
  assert.deepEqual(defaultModel(def('search'), models), { engine: 'codex', model: 'gpt-6-luna' });
  const none: Models = { claude: ['opus', 'sonnet', 'haiku'], codex: [] };
  assert.deepEqual(defaultModel(def('builder'), none), { engine: 'claude', model: 'sonnet' });
  assert.deepEqual(defaultModel(def('search'), none), { engine: 'claude', model: 'haiku' });
  // A role's preferred model that is not on offer falls back to one that is, not to a name that may not exist.
  assert.deepEqual(defaultModel(def('builder'), { claude: ['opus'], codex: [] }), { engine: 'claude', model: 'opus' });
});

test('a new builder waits for approval and a second node of a role is numbered', () => {
  const f = line();
  assert.equal(f.nodes[2]!.checkpoint, true);
  assert.equal(f.nodes[1]!.checkpoint, false);
  assert.equal(newNode(f, def('builder'), models, at).name, 'Builder 2');
  assert.equal(newNode(f, def('search'), models, at).name, 'Search');
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

test('retry edges: a verdict agent to any earlier agent, and several places are allowed', () => {
  let f = line(); // n1 orchestrator, n2 researcher, n3 builder, n4 judge
  assert.equal(connectionProblem(f, 'n4', 'n3', 'retry'), null);
  assert.match(connectionProblem(f, 'n3', 'n3', 'retry')!, /itself/);
  assert.match(connectionProblem(f, 'n2', 'n3', 'retry')!, /verdict/);
  // An earlier agent that only reads is a fine place to send work back to: a wrong design goes back to its author.
  assert.equal(connectionProblem(f, 'n4', 'n2', 'retry'), null);
  assert.equal(connectionProblem(f, 'n4', 'n1', 'retry'), null);
  // A judge can have a second place to send work back to, but not the same one twice.
  f = addEdge(f, 'n4', 'n3', 'retry');
  assert.equal(connectionProblem(f, 'n4', 'n2', 'retry'), null);
  assert.match(connectionProblem(f, 'n4', 'n3', 'retry')!, /Already/);
  assert.equal(flowProblem(addEdge(f, 'n4', 'n2', 'retry')), null);
  // An agent that runs after the judge cannot be sent back to.
  let g = line();
  g = { ...g, nodes: [...g.nodes, newNode(g, def('analyst'), models, at)] };
  g = addEdge(g, 'n4', 'n5', 'forward');
  assert.match(connectionProblem(g, 'n4', 'n5', 'retry')!, /before/);
});

test('layers and tidy lay a flow out left to right', () => {
  const f = line();
  assert.deepEqual(layers(f), [['n1'], ['n2'], ['n3'], ['n4']]);
  const t = tidy(f);
  assert.deepEqual(t.nodes.map((n) => n.x), [0, 280, 560, 840]);
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
  f = { ...f, nodes: [...f.nodes, newNode(f, def('search'), models, at)] };
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
  assert.match(flowProblem(addEdge(line(), 'n2', 'n3', 'retry'))!, /verdict/);
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

const single = { text: 'Which database?', recommended: 'SQLite', options: ['SQLite', 'Postgres', 'Files'], multiple: false };
const several = { text: 'Which matter most?', recommended: 'Views', options: ['Views', 'Labels', 'Snippets', 'Calendar'], multiple: true };
const wordy = { text: 'Anything else?', recommended: 'No' };

test('a question starts with the suggestion ticked, so continuing takes it', () => {
  assert.deepEqual(startingAnswer(single), { chosen: ['SQLite'], other: false, text: '' });
  assert.deepEqual(answersFor([single, several, wordy], {}), ['SQLite', 'Views', 'No']);
});

test('choosing an option answers with its words, and one choice replaces another', () => {
  let p = startingAnswer(single);
  p = choose(single, p, 'Postgres');
  assert.equal(answerFor(single, p), 'Postgres');
  p = choose(single, p, 'Files');
  assert.deepEqual(p.chosen, ['Files']);
});

test('a multiple-choice question joins what was ticked, in the order offered', () => {
  let p = startingAnswer(several); // Views
  p = choose(several, p, 'Snippets');
  p = choose(several, p, 'Labels');
  assert.equal(answerFor(several, p), 'Views; Labels; Snippets');
  p = choose(several, p, 'Views'); // untick
  assert.equal(answerFor(several, p), 'Labels; Snippets');
  // Unticking everything falls back to the suggestion rather than sending nothing.
  p = choose(several, choose(several, p, 'Labels'), 'Snippets');
  assert.equal(answerFor(several, p), 'Views');
});

test('Something else sends what was typed; blank takes the suggestion', () => {
  let p = writeInstead(single, startingAnswer(single));
  assert.deepEqual(p.chosen, [], 'a single-choice question drops its tick');
  assert.equal(answerFor(single, p), 'SQLite', 'nothing typed yet: the suggestion');
  p = { ...p, text: '  DuckDB, because it is embedded  ' };
  assert.equal(answerFor(single, p), 'DuckDB, because it is embedded');
  // Picking an option again puts the typed answer down.
  assert.equal(answerFor(single, choose(single, p, 'Files')), 'Files');
});

test('in a multiple-choice question the words are added to the ticks', () => {
  let p = choose(several, startingAnswer(several), 'Labels'); // Views; Labels
  p = { ...writeInstead(several, p), text: 'Search' };
  assert.equal(answerFor(several, p), 'Views; Labels; Search');
});

test('a question with no choices is a text box, and one with a single option is not a choice', () => {
  assert.equal(hasChoices(wordy), false);
  assert.equal(hasChoices({ ...single, options: ['Only'] }), false);
  assert.equal(answersFor([wordy], { 0: { chosen: [], other: false, text: ' Maybe later ' } })[0], 'Maybe later');
  assert.equal(answersFor([wordy], {})[0], 'No');
});

test('a stale tick that is no longer an option is never sent', () => {
  const p = { chosen: ['Mongo'], other: false, text: '' };
  assert.equal(answerFor(single, p), 'SQLite');
});

test('a finished run says which branch holds each builder’s work', () => {
  const r = runOf(line(), { state: 'done' });
  assert.deepEqual(builderBranches(r), []);
  r.nodes = { n3: { status: 'done', attempts: 2, output: null, error: null, transcript: [], tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } } };
  assert.deepEqual(builderBranches(r), [{ node: 'n3', name: 'Builder', branch: 'ttm-orch/r1-n3' }]);
});

const nodeRun = (status: 'pending' | 'running' | 'done', extra: object = {}) => ({
  status, attempts: 1, output: null, error: null, transcript: [], tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, ...extra,
});

test('an agent waiting for approval is shown even while its sibling is still working', () => {
  // orch -> a, b, both asking first; a's input is done, b is not started because orch is the only thing done.
  const f = line(); // n1 orch -> n2 researcher -> n3 builder (asks) -> n4 judge
  const r = runOf(f, { state: 'running' });
  r.nodes = { n1: nodeRun('done'), n2: nodeRun('running'), n3: nodeRun('pending'), n4: nodeRun('pending') };
  assert.deepEqual(awaitingApproval(r), [], 'the builder is not ready until the researcher is done');
  r.nodes.n2 = nodeRun('done');
  assert.deepEqual(awaitingApproval(r), ['n3'], 'ready, and it asks first');
  r.approved = ['n3'];
  assert.deepEqual(awaitingApproval(r), []);
});

test('a downshifted model shows what it replaced', () => {
  const n = line().nodes[0]!; // opus
  assert.deepEqual(modelShown(n), { label: 'opus', cheaper: false });
  assert.deepEqual(modelShown(n, { modelUsed: 'opus' }), { label: 'opus', cheaper: false });
  assert.deepEqual(modelShown(n, { modelUsed: 'sonnet' }), { label: 'opus → sonnet', cheaper: true });
});

test('durations read naturally', () => {
  assert.deepEqual([4, 59, 60, 83, 3599, 3600, 3725].map(fmtDuration), ['4s', '59s', '1m 00s', '1m 23s', '59m 59s', '1h 00m', '1h 02m']);
});

test('a run summary counts what happened', () => {
  const f = line();
  const r = runOf(f, { state: 'done' });
  r.created = 100;
  r.finished = 183;
  r.retriesUsed = 1;
  r.nodes = {
    n1: nodeRun('done', { modelUsed: 'sonnet', tokens: { input: 10, output: 5, cacheRead: 80, cacheWrite: 5 } }),
    n3: nodeRun('done', { cacheBreaks: 2, tokens: { input: 10, output: 5, cacheRead: 80, cacheWrite: 5 } }),
  };
  const s = summary(r);
  assert.equal(s.duration, 83);
  assert.equal(s.cheaper, 1, 'only the orchestrator ran on something other than what the flow asked for');
  assert.equal(s.cacheBreaks, 2);
  assert.equal(cacheBreakCount(r), 2);
  assert.equal(s.sentBack, 1);
  assert.equal(s.tokens, 210, 'two agents of 100 tokens each, plus 10 of overhead (the questions)');
  assert.equal(summary({ ...r, finished: null }).duration, null);
});

// ---------------------------------------------------------------------------- roles

const customReviewer = (capability: 'read' | 'inspect' | 'write', output: 'plan' | 'report' | 'change' | 'verdict') =>
  ({ label: 'Security reviewer', capability, output }) as const;

test('a custom role is treated by what it may do, not by its name', () => {
  let f = line(); // n1 orchestrator, n2 researcher, n3 builder, n4 judge
  const reviewer = newCustomNode(f, customReviewer('inspect', 'verdict'), 'You review the change for security problems and report each with evidence.', models, at);
  assert.equal(reviewer.role, 'custom');
  assert.equal(reviewer.name, 'Security reviewer');
  assert.equal(reviewer.checkpoint, false, 'it changes nothing, so it does not ask first');
  f = { ...f, nodes: [...f.nodes, reviewer] };
  f = addEdge(f, 'n3', reviewer.id, 'forward');
  // It gives a verdict, so it may send work back to the builder, exactly like the built-in judge.
  assert.equal(connectionProblem(f, reviewer.id, 'n3', 'retry'), null);
  assert.equal(decides(reviewer), true);
  assert.equal(writes(reviewer), false);
  assert.equal(roleLabel(reviewer), 'Security reviewer');
});

test('a custom writer asks first and can be sent work back, and a custom reader cannot send it', () => {
  const f = line();
  const writer = newCustomNode(f, customReviewer('write', 'plan'), 'You make the change described and report exactly what you changed.', models, at);
  assert.equal(writer.checkpoint, true);
  assert.equal(writer.custom!.output, 'change', 'a writer reports a change whatever was asked for');
  assert.equal(writes(writer), true);
  const reader = newCustomNode(f, customReviewer('read', 'report'), 'You look things up and report what you found, with where you found it.', models, at);
  const g = { ...f, nodes: [...f.nodes, reader] };
  assert.match(connectionProblem(g, reader.id, 'n3', 'retry')!, /verdict/);
});

test('the same consistency rules as the server', () => {
  assert.equal(consistent('write', 'verdict'), 'change');
  assert.equal(consistent('write', 'report'), 'change');
  assert.equal(consistent('read', 'change'), 'report');
  assert.equal(consistent('inspect', 'change'), 'report');
  assert.equal(consistent('inspect', 'verdict'), 'verdict');
  assert.equal(consistent('read', 'plan'), 'plan');
});

test('a reviewer can never be made able to write, whatever it is asked for', () => {
  for (const out of ['plan', 'report', 'change', 'verdict'] as const) {
    assert.notEqual(consistent('read', out), 'change');
    assert.notEqual(consistent('inspect', out), 'change');
  }
  // The only way to a verdict is without write access.
  assert.notEqual(consistent('write', 'verdict'), 'verdict');
});

test('the catalog drives roles, labels and icons, and an unknown role is safe', () => {
  setCatalog([{ ...def('builder'), id: 'welder', label: 'Welder', blurb: 'Joins things', icon: 'hammer' }]);
  try {
    assert.equal(roleLabel({ role: 'welder' }), 'Welder');
    assert.equal(roleIconName({ role: 'welder' }), 'hammer');
    assert.equal(writes({ role: 'welder' }), true);
    // A role the page does not know reads and reports: the safe default, never write.
    assert.equal(writes({ role: 'mystery' }), false);
    assert.equal(roleLabel({ role: 'mystery' }), 'Agent');
  } finally {
    setCatalog([]); // back to the built-in six
  }
  assert.equal(roleLabel({ role: 'builder' }), 'Builder');
  assert.equal(roleIconName({ custom: { label: 'X', capability: 'read', output: 'report' }, role: 'custom' }), 'wand');
});

test('every built-in role the page knows is offered in a stage, with the stages in flow order', () => {
  const stages = STAGES.map((s) => s.id);
  for (const d of getCatalog()) assert.ok(stages.includes(d.stage), d.id);
  assert.deepEqual(stages, ['plan', 'gather', 'build', 'check', 'deliver']);
});

test('a flow with a custom role that has no definition is not accepted by the server, and the page agrees it is a custom role', () => {
  const f = line();
  const n = newCustomNode(f, customReviewer('read', 'report'), 'You look things up and report what you found, with where you found it.', models, at);
  assert.equal(flowProblem({ ...f, nodes: [...f.nodes, n] }), null);
});
