import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  awaitingApproval, cacheBreakCount, fmtDuration, modelShown, summary,
  flowEntries, runsLabel, runsOf, whenLabel,
  addEdge, answerFor, answersFor, cacheHit, choose, hasChoices, startingAnswer, writeInstead, connectionProblem, defaultModel, emptyFlow, flowProblem, fmtTokens, freeId, gauges, layers,
  builderBranches, consistent, decides, getCatalog, newCustomNode, newNode, pauseInfo, removeNode, roleDef, roleIconName, roleLabel, runOrder, runTokens,
  setCatalog, STAGES, statusLabel, tidy, writes, duplicateNode, isFinished, logView, skillCount, clock, liveLines,
  canCallDirectly, modelKey, modelPatch, openRouterChoices, parseModelKey, priceNote, runCost, usdLabel,
} from './model.ts';
import type { Flow, FlowNode, Models, Role, Run, RunSummary, UsageSnapshot } from './types.ts';

const models: Models = { claude: ['opus', 'sonnet', 'haiku'], codex: ['gpt-6.1-sol', 'gpt-6-luna'], openrouter: [] };
const at = { x: 0, y: 0 };
const LIMITS = { claudePct: 15, codexPct: 10, stopWindowPct: 85 };
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
  const none: Models = { claude: ['opus', 'sonnet', 'haiku'], codex: [], openrouter: [] };
  assert.deepEqual(defaultModel(def('builder'), none), { engine: 'claude', model: 'sonnet' });
  assert.deepEqual(defaultModel(def('search'), none), { engine: 'claude', model: 'haiku' });
  // A role's preferred model that is not on offer falls back to one that is, not to a name that may not exist.
  assert.deepEqual(defaultModel(def('builder'), { claude: ['opus'], codex: [], openrouter: [] }), { engine: 'claude', model: 'opus' });
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
  // Limits are no longer part of a flow, so an old flow's own numbers cannot stop it being saved.
  const old = line();
  old.budget.claudePct = 0;
  assert.equal(flowProblem(old), null);
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
  assert.equal(b.primary!.waive, true, 'carrying on past a limit has to say so, or the limit stops it again');
  assert.equal(b.limits, true);
  assert.equal(r.primary!.waive, undefined);

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
  const g = gauges(f, run, usage(35, 2), LIMITS);
  assert.deepEqual(g.map((x) => [x.engine, x.share, x.tone]), [['claude', 5, 'ok'], ['codex', 2, 'ok']]);
  assert.equal(gauges(f, run, usage(43, 2), LIMITS)[0]!.tone, 'warn'); // 13 of 15
  assert.equal(gauges(f, run, usage(45, 2), LIMITS)[0]!.tone, 'bad'); // 15 of 15
  assert.equal(gauges(f, run, usage(86, 2), LIMITS)[0]!.tone, 'bad'); // past the stop line
  // Started high, used little, but the window itself is close to the stop line.
  const high = { ...run, baseline: { claude: 70, codex: 0 } };
  assert.equal(gauges(f, high, usage(76, 2), LIMITS)[0]!.tone, 'warn');
  assert.equal(gauges(f, high, usage(60, 2), LIMITS)[0]!.tone, 'ok');
  // The limits are whatever is set now: with more room, the same use is no longer a problem.
  const roomy = { ...LIMITS, claudePct: 40, stopWindowPct: 95 };
  assert.equal(gauges(f, run, usage(45, 2), roomy)[0]!.tone, 'ok');
  assert.equal(gauges(f, run, usage(45, 2), roomy)[0]!.budget, 40);
  // No run yet: only the window itself is known.
  assert.equal(gauges(f, null, usage(35, 2), LIMITS)[0]!.share, null);
  assert.equal(gauges(f, null, null, LIMITS)[0]!.used, null);
  // Only engines the flow uses are shown.
  const claudeOnly = { ...f, nodes: f.nodes.filter((n) => n.engine === 'claude') };
  assert.deepEqual(gauges(claudeOnly, null, null, LIMITS).map((x) => x.engine), ['claude']);
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

// ---------------------------------------------------------------------------- flows and their executions

const summary_ = (id: string, flowId: string, created: number, flowName = flowId): RunSummary =>
  ({ id, flowId, flowName, goal: `goal ${id}`, status: { state: 'done' }, created, tokens: 100 });

test('a flow lists its own executions, newest first, and no one else’s', () => {
  const runs = [summary_('a', 'f1', 10), summary_('b', 'f2', 20), summary_('c', 'f1', 30)];
  assert.deepEqual(runsOf(runs, 'f1').map((r) => r.id), ['c', 'a']);
  assert.deepEqual(runsOf(runs, 'f2').map((r) => r.id), ['b']);
  assert.deepEqual(runsOf(runs, 'none'), []);
});

test('the sidebar lists flows, the most recently run first, then flows never run by name', () => {
  const f = (id: string, name: string): Flow => ({ ...emptyFlow(id, name), nodes: [newNode(emptyFlow(id, name), def('builder'), models, at)] });
  const saved = [f('z', 'Zebra'), f('a', 'Alpha'), f('m', 'Mid')];
  const entries = flowEntries(saved, [summary_('r1', 'm', 50), summary_('r2', 'z', 90), summary_('r3', 'm', 70)]);
  assert.deepEqual(entries.map((e) => e.name), ['Zebra', 'Mid', 'Alpha']);
  assert.equal(entries[1]!.runs.length, 2);
  assert.equal(entries[1]!.last!.id, 'r3');
  assert.equal(entries[2]!.last, null, 'never run');
  assert.deepEqual(entries.map((e) => e.saved), [true, true, true]);
  assert.equal(entries[0]!.agents, 1);
});

test('executions of a flow that is no longer saved still appear, marked as runs only', () => {
  const entries = flowEntries([], [summary_('r1', 'gone', 10, 'Old flow'), summary_('r2', 'gone', 20, 'Old flow renamed')]);
  assert.equal(entries.length, 1);
  assert.deepEqual({ saved: entries[0]!.saved, agents: entries[0]!.agents, name: entries[0]!.name, n: entries[0]!.runs.length }, { saved: false, agents: null, name: 'Old flow renamed', n: 2 });
});

test('a saved flow takes its name from the flow, not from its old executions', () => {
  const saved = [{ ...emptyFlow('f1', 'Current name'), nodes: [] }];
  const [e] = flowEntries(saved, [summary_('r1', 'f1', 10, 'An older name')]);
  assert.equal(e!.name, 'Current name');
});

test('run counts read naturally', () => {
  assert.deepEqual([0, 1, 2, 12].map(runsLabel), ['Never run', '1 run', '2 runs', '12 runs']);
});

test('an execution’s time is told as today, yesterday, or a date', () => {
  const now = new Date(2026, 9, 15, 20, 0); // 15 Oct 2026, 20:00 local
  const at = (y: number, mo: number, d: number, h: number, mi: number) => Math.floor(new Date(y, mo, d, h, mi).getTime() / 1000);
  assert.equal(whenLabel(at(2026, 9, 15, 19, 4), now), 'Today 19:04');
  assert.equal(whenLabel(at(2026, 9, 14, 8, 5), now), 'Yesterday 08:05');
  assert.equal(whenLabel(at(2026, 9, 12, 14, 20), now), '12 Oct 14:20');
  assert.equal(whenLabel(at(2025, 11, 31, 23, 59), now), '31 Dec 2025 23:59', 'another year says which');
  // Just after midnight is still "Today" even though it is less than a day ago.
  assert.equal(whenLabel(at(2026, 9, 15, 0, 5), new Date(2026, 9, 15, 0, 30)), 'Today 00:05');
});

test('a duplicated agent is a separate, unconnected copy beside the original', () => {
  const f = line();
  const builder = f.nodes[2]!;
  const out = duplicateNode(f, builder.id)!;
  assert.equal(out.flow.nodes.length, f.nodes.length + 1);
  assert.notEqual(out.node.id, builder.id);
  assert.equal(out.node.name, 'Builder 2');
  assert.equal(out.node.role, builder.role);
  assert.equal(out.node.systemPrompt, builder.systemPrompt);
  assert.deepEqual([out.node.x, out.node.y], [builder.x + 40, builder.y + 40]);
  assert.equal(out.flow.edges.length, f.edges.length);
  assert.equal(duplicateNode(f, 'nope'), null);
});

test('only an execution that has stopped for good counts as finished', () => {
  assert.equal(isFinished({ state: 'done' }), true);
  assert.equal(isFinished({ state: 'cancelled' }), true);
  for (const s of ['running', 'grilling', 'paused']) assert.equal(isFinished({ state: s }), false);
});

const entry = (kind: string, node: string | null, title = 't') => ({ at: 0, node, kind, title }) as import('./types.ts').LogEntry;

test('the log can be narrowed to skills and sub-agents, or to one agent', () => {
  const log = [entry('request', 'a'), entry('skill', 'a', 'a used the skill pdf'), entry('decision', null), entry('subagent', 'b'), entry('response', 'b')];
  assert.equal(logView(log, 'all').length, 5);
  assert.deepEqual(logView(log, 'skills').map((e) => e.kind), ['skill', 'subagent']);
  assert.deepEqual(logView(log, 'node:b').map((e) => e.kind), ['subagent', 'response']);
  assert.equal(logView(log, 'node:zzz').length, 0);
  assert.equal(skillCount(log), 2);
  assert.equal(skillCount([]), 0);
});

test('a log time is shown as the time of day', () => {
  const t = new Date(2026, 9, 15, 4, 5, 9).getTime() / 1000;
  assert.equal(clock(t), '04:05:09');
});

test('the log shows what each working agent last did, and nothing for the rest', () => {
  const f = line();
  const [a, b] = f.nodes as [FlowNode, FlowNode];
  const state = (status: string, transcript: string[]) => ({ status, attempts: 1, output: null, error: null, tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, transcript }) as unknown as Run['nodes'][string];
  const run = { flow: f, nodes: { [a.id]: state('done', ['x']), [b.id]: state('running', ['→ Read a.rs', '→ Grep foo']) } } as unknown as Run;
  assert.deepEqual(liveLines(run), [{ node: b.id, line: '→ Grep foo' }]);
  run.nodes[b.id] = state('running', []);
  assert.deepEqual(liveLines(run), [{ node: b.id, line: 'starting…' }]);
});

const orModels: Models = {
  ...models,
  openrouter: [
    { account: 'a1b2', accountName: 'Main', id: 'anthropic/claude-haiku-5.5', driver: 'claude', priceIn: 1, priceOut: 5, tools: true, direct: false },
    { account: 'a1b2', accountName: 'Main', id: 'openai/gpt-6-luna:free', driver: 'codex', priceIn: 0, priceOut: 0, tools: true, direct: false },
    { account: 'z9', accountName: 'Plain', id: 'vendor/think', driver: 'codex', priceIn: 1, priceOut: 2, tools: false, direct: true },
  ],
};

test('the model picker tells your plan\'s models from OpenRouter ones, even with colons in the id', () => {
  assert.equal(modelKey({ model: 'sonnet' }), 'plan:sonnet');
  assert.equal(modelKey({ model: 'openai/gpt-6-luna:free', account: 'a1b2' }), 'or:a1b2:openai/gpt-6-luna:free');
  assert.deepEqual(parseModelKey('plan:sonnet'), { account: null, model: 'sonnet' });
  assert.deepEqual(parseModelKey('or:a1b2:openai/gpt-6-luna:free'), { account: 'a1b2', model: 'openai/gpt-6-luna:free' });
  assert.deepEqual(parseModelKey('sonnet'), { account: null, model: 'sonnet' }, 'an old bare value still reads');
  const k = modelKey({ model: 'x/y:z', account: 'acct' });
  assert.deepEqual(parseModelKey(k), { account: 'acct', model: 'x/y:z' });
});

test('choosing an OpenRouter model brings its account and its CLI; choosing one of yours clears the account', () => {
  assert.deepEqual(modelPatch('or:a1b2:anthropic/claude-haiku-5.5', orModels), { model: 'anthropic/claude-haiku-5.5', account: 'a1b2', direct: false, engine: 'claude' });
  assert.deepEqual(modelPatch('or:a1b2:openai/gpt-6-luna:free', orModels), { model: 'openai/gpt-6-luna:free', account: 'a1b2', direct: false, engine: 'codex' });
  assert.deepEqual(modelPatch('plan:haiku', orModels), { model: 'haiku', account: null, direct: false }, 'a plan model also ends a direct call');
  assert.deepEqual(modelPatch('or:z9:vendor/think', orModels), { model: 'vendor/think', account: 'z9', direct: true }, 'a direct model leaves the engine alone');
  assert.deepEqual(modelPatch('or:a1b2:vendor/unlisted', orModels), { model: 'vendor/unlisted', account: 'a1b2' }, 'a model not on offer leaves the engine alone');
});

test('a node whose account is gone still shows its model, marked unavailable, so it is not silently changed', () => {
  const n = { ...newNode(emptyFlow('f', 'F'), def('builder'), models, at), account: 'gone', model: 'x/y' } as FlowNode;
  const oneAccount: Models = { ...orModels, openrouter: orModels.openrouter.filter((m) => m.account === 'a1b2') };
  const c = openRouterChoices(n, oneAccount);
  assert.deepEqual(c[0], { key: 'or:gone:x/y', label: 'x/y (account unavailable)', offered: false, direct: false });
  assert.equal(c.length, 3);
  assert.equal(c[1]!.label, 'anthropic/claude-haiku-5.5', 'one account: its name adds nothing');
  const two: Models = { ...orModels, openrouter: [...oneAccount.openrouter, { account: 'c3d4', accountName: 'Work', id: 'google/gemini-3', driver: 'codex', priceIn: 1, priceOut: 2, tools: true, direct: false }] };
  assert.equal(openRouterChoices(n, two)[1]!.label, 'Main · anthropic/claude-haiku-5.5', 'two accounts: say which');
  const plain = newNode(emptyFlow('f', 'F'), def('builder'), models, at);
  assert.equal(openRouterChoices(plain, orModels).length, 3, 'a node on your plan adds nothing');
});

test('a run costs only what OpenRouter charged: agents on your plan are estimates and are not counted', () => {
  let f = emptyFlow('f', 'F');
  f = { ...f, nodes: [newNode(f, def('orchestrator'), models, at)] };
  f = { ...f, nodes: [...f.nodes, { ...newNode(f, def('builder'), models, at), id: 'or1', account: 'a1b2' }] };
  const run = {
    flow: f,
    nodes: { [f.nodes[0]!.id]: { costUsd: 0.5 }, or1: { costUsd: 0.0123 } },
  } as unknown as Run;
  assert.equal(runCost(run), 0.0123);
});

test('dollars and prices read plainly', () => {
  assert.equal(usdLabel(0), '$0.00');
  assert.equal(usdLabel(0.00004), '<$0.0001');
  assert.equal(usdLabel(0.004), '$0.0040');
  assert.equal(usdLabel(0.000425), '$0.0004');
  assert.equal(usdLabel(0.0123), '$0.012');
  assert.equal(usdLabel(0.176), '$0.176');
  assert.equal(usdLabel(2.5), '$2.500'.replace('.500', '.50'));
  assert.equal(usdLabel(12.5), '$12.50');
  assert.equal(priceNote({ priceIn: 1, priceOut: 5 }), '$1 in · $5 out, per million tokens');
  assert.equal(priceNote({ priceIn: 0.2, priceOut: 1.2 }), '$0.20 in · $1.20 out, per million tokens');
  assert.equal(priceNote({ priceIn: 0, priceOut: 0 }), 'free in · free out, per million tokens');
  assert.equal(priceNote({ priceIn: null, priceOut: null }), 'No price on file');
});

test('only a role that needs no tools can call its model directly', () => {
  const n = (role: Role) => newNode(emptyFlow('f', 'F'), def(role), models, at);
  assert.equal(canCallDirectly(n('orchestrator')), true);
  assert.equal(canCallDirectly(n('researcher')), true);
  assert.equal(canCallDirectly(n('builder')), false);
  assert.equal(canCallDirectly(n('judge')), false, 'a judge runs the checks');
});
