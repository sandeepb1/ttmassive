import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  awaitingApproval, cacheBreakCount, fmtDuration, modelShown, summary,
  flowEntries, runsLabel, runsOf, whenLabel,
  addEdge, answerFor, answersFor, cacheHit, choose, hasChoices, startingAnswer, writeInstead, connectionProblem, defaultModel, emptyFlow, flowProblem, fmtTokens, freeId, gauges, layers,
  builderBranches, consistent, decides, getCatalog, newCustomNode, newNode, pauseInfo, removeNode, roleDef, roleIconName, roleLabel, runOrder, runTokens,
  setCatalog, STAGES, statusLabel, tidy, writes, duplicateNode, isFinished, logView, skillCount, clock, liveLines,
  offered, resumeLabel, untilLabel, rerunRequest, unnumbered, waitingQuestions, providerOf, providerPatch, providersFor, orGauges, hasLink, ladderSummary, nextRung, rungFromKey, rungKey, withRoutingChange, asks, edgeLabel, rounds, turnFacts, turnLength, turnTone, turnsOf, canCallDirectly, runCredits, modelKey, modelPatch, openRouterChoices, parseModelKey, priceNote, runCost, usdLabel,
} from './model.ts';
import { DEFAULT_LIMITS } from './types.ts';
import type { Rung, Turn, Capability, Flow, FlowNode, Models, Output, Role, Run, RunSummary, UsageSnapshot } from './types.ts';

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
  assert.match(r.detail, /1 round\b/);
  assert.equal(r.moreRounds, 2, 'it can be allowed to go on');
  assert.match(r.title, /sent back too many times/);
  const g = { ...f, nodes: f.nodes.map((n) => (n.id === 'n4' ? { ...n, custom: { label: 'Griller', capability: 'read' as const, output: 'questions' as const, instructions: 'x' } } : n)) };
  const gi = pauseInfo(runOf(g, { state: 'paused', pause: { kind: 'retriesExhausted', node: 'n4' } }))!;
  assert.match(gi.title, /still has questions/, 'a griller has questions left, not a verdict');
  assert.match(gi.primary!.label, /without more questions/);
  assert.equal(gi.moreRounds, 2);

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

test('agents that look things up start with web access on, and a run adds up its credits', () => {
  const f = emptyFlow('f', 'F');
  const researcher = newNode(f, { ...def('researcher'), web: true }, models, at);
  const builder = newNode(f, { ...def('builder'), web: false }, models, at);
  assert.equal(researcher.web, true);
  assert.equal(builder.web, false);
  const run = { nodes: { a: { credits: 3 }, b: { credits: 2 }, c: {} } } as unknown as Run;
  assert.equal(runCredits(run), 5, 'an agent that used none counts as none');
});

test('an agent that asks questions can send work back, to one place only, with the usual limits', () => {
  let f = emptyFlow('f', 'F');
  for (const r of ['orchestrator', 'researcher', 'griller'] as Role[]) f = { ...f, nodes: [...f.nodes, newNode(f, { ...def(r === 'griller' ? 'orchestrator' : r), id: r }, models, at)] };
  const [a, b, g] = f.nodes.map((n) => n.id) as [string, string, string];
  // The page's catalog stands in for the server's: a griller asks questions, an orchestrator does not.
  const gNode = f.nodes.find((n) => n.id === g)!;
  assert.equal(asks({ ...gNode, role: 'griller', custom: { label: 'G', capability: 'read', output: 'questions' } }), true);
  assert.equal(asks(f.nodes[0]!), false);
  f = { ...f, nodes: f.nodes.map((n) => (n.id === g ? { ...n, custom: { label: 'Griller', capability: 'read' as Capability, output: 'questions' as Output }, role: 'custom' } : n)) };
  f = addEdge(f, a, b, 'forward');
  f = addEdge(f, b, g, 'forward');
  assert.equal(connectionProblem(f, g, a, 'retry'), null, 'a griller may send work back like a judge');
  f = addEdge(f, g, a, 'retry');
  assert.deepEqual(f.edges.at(-1), { id: 'e3', from: g, to: a, kind: 'retry', maxIterations: 2, onCap: 'pause', answeredBy: 'user' });
  assert.deepEqual(f.edges[0], { id: 'e1', from: a, to: b, kind: 'forward' }, 'a plain edge carries no limits');
  assert.match(connectionProblem(f, g, b, 'retry') ?? '', /only one place/, 'one way back');
  assert.match(connectionProblem(f, a, b, 'retry') ?? '', /verdict or asks questions/);
  // The card only has a place to receive a send-back on an agent that does not send work back itself.
  const griller = f.nodes.find((n) => n.id === g)!;
  const g2: FlowNode = { ...griller, id: 'g2' };
  const before = { ...f, nodes: [...f.nodes, g2], edges: [{ id: 'x1', from: a, to: 'g2', kind: 'forward' as const }, { id: 'x2', from: 'g2', to: g, kind: 'forward' as const }] };
  assert.match(connectionProblem(before, g, 'g2', 'retry') ?? '', /does not itself send work back/);
  assert.equal(connectionProblem(before, g, a, 'retry'), null, 'an ordinary agent can be sent work');
});

test('a send-back edge says how often it may be used, and in a run how often it has been', () => {
  const e = { id: 'e', from: 'a', to: 'b', kind: 'retry' as const, maxIterations: 3 };
  assert.equal(edgeLabel(e, null), 'send back ×3');
  assert.equal(edgeLabel({ ...e, maxIterations: undefined }, null), 'send back ×2', 'an older edge has the usual limit');
  assert.equal(edgeLabel(e, { iterations: { e: 1 } } as unknown as Run), 'send back 1/3');
  assert.equal(edgeLabel(e, { } as unknown as Run), 'send back 0/3');
  assert.equal(edgeLabel({ ...e, kind: 'forward' }, null), undefined);
});

test('a run waiting for the user\'s answers says so and offers to answer', () => {
  const f = emptyFlow('f', 'F');
  const run = { flow: { ...f, nodes: [{ ...newNode(f, def('orchestrator'), models, at), id: 'g', name: 'Griller' }] }, status: { state: 'paused', pause: { kind: 'questions', node: 'g' } }, nodes: {} } as unknown as Run;
  const info = pauseInfo(run)!;
  assert.equal(info.title, 'Griller has questions for you');
  assert.equal(info.primary?.action, 'answer');
});

const turn = (o: Partial<Turn>): Turn => ({
  n: 1, node: 'a', attempt: 1, round: 0, started: 1_000_000, ended: 1_004_000, engine: 'claude', model: 'sonnet', account: null,
  tokens: { input: 100, output: 50, cacheRead: 0, cacheWrite: 0 }, costUsd: 0, credits: 0, ...o,
});

test('turns fall into rounds, and a round says why the next began', () => {
  const ts = [
    turn({ n: 1, node: 'plan' }),
    turn({ n: 2, node: 'build' }),
    turn({ n: 3, node: 'judge', decision: 'Judge rejected the work and sent it back to Builder (send-back 1 of 2)' }),
    turn({ n: 4, node: 'build', round: 1, attempt: 2 }),
    turn({ n: 5, node: 'judge', round: 1, attempt: 2, decision: 'Judge approved the work: carrying on' }),
  ];
  const r = rounds(ts);
  assert.deepEqual(r.map((x) => [x.round, x.turns.length]), [[0, 3], [1, 2]]);
  assert.match(r[0]!.because ?? '', /sent it back to Builder/);
  assert.equal(r[1]!.because, null, 'the last round has nothing after it');
  assert.deepEqual(rounds([]), []);
  assert.deepEqual(rounds([turn({})]).map((x) => x.turns.length), [1]);
});

test('a turn says how long it took and how it went', () => {
  assert.equal(turnLength(turn({ started: 0, ended: 4000 })), '4s');
  assert.equal(turnLength(turn({ started: 0, ended: 64_000 })), '1m 04s');
  assert.equal(turnLength(turn({ started: 5000, ended: 1000 })), '0s', 'never negative');
  assert.equal(turnTone(turn({})), 'ok');
  assert.equal(turnTone(turn({ failed: 'claude exited' })), 'bad');
  assert.equal(turnTone(turn({ decision: 'Judge rejected the work and sent it back to Builder' })), 'warn');
  assert.equal(turnTone(turn({ decision: 'Griller asked 2 questions; waiting for your answers' })), 'warn');
  assert.equal(turnTone(turn({ decision: 'Judge approved the work: carrying on' })), 'ok');
});

test('the facts line names the CLI, model, time and tokens, and money only where there was a charge', () => {
  assert.deepEqual(turnFacts(turn({}), 'Planner'), ['Claude Code', 'sonnet', '4s', '150 tok']);
  assert.deepEqual(turnFacts(turn({ engine: 'codex', model: 'gpt-6-luna', account: 'a', costUsd: 0.0012, credits: 2, attempt: 3, effort: 'high' }), 'Builder'), ['Codex', 'gpt-6-luna', 'high effort', '4s', '150 tok', '$0.0012', '2 web credits', 'Builder, try 3']);
  assert.equal(turnFacts(turn({ engine: 'direct' }), 'x')[0], 'direct call');
  assert.ok(!turnFacts(turn({ costUsd: 0.5 }), 'x').some((f) => f.startsWith('$')), 'an estimate for your own plan is not shown as money');
});

test('the timeline can be narrowed to one agent', () => {
  const ts = [turn({ n: 1, node: 'a' }), turn({ n: 2, node: 'b' }), turn({ n: 3, node: 'a' })];
  assert.deepEqual(turnsOf(ts, 'a').map((t) => t.n), [1, 3]);
  assert.equal(turnsOf(ts, null).length, 3);
});

const rungModels: Models = {
  claude: ['opus', 'sonnet', 'haiku'], codex: ['gpt-6-luna'], web: false,
  openrouter: [
    { account: 'a1', accountName: 'Main', id: 'anthropic/claude-haiku-5.5', driver: 'claude', priceIn: 1, priceOut: 5, tools: true, direct: false },
    { account: 'a1', accountName: 'Main', id: 'openai/gpt-6-luna', driver: 'codex', priceIn: 0.2, priceOut: 1.2, tools: true, direct: false },
    { account: 'z9', accountName: 'Plain', id: 'vendor/think', driver: 'codex', priceIn: 1, priceOut: 2, tools: false, direct: true },
  ],
};

test('a rung is picked from the same list as a model, and its CLI follows its model', () => {
  assert.equal(rungKey({ engine: 'codex', model: 'gpt-6-luna', account: null }), 'codex:gpt-6-luna');
  assert.equal(rungKey({ engine: 'claude', model: 'a/b:free', account: 'a1' }), 'or:a1:a/b:free');
  assert.deepEqual(rungFromKey('claude:opus', rungModels), { engine: 'claude', model: 'opus', account: null, effort: null });
  assert.deepEqual(rungFromKey('codex:gpt-6-luna', rungModels, 'high'), { engine: 'codex', model: 'gpt-6-luna', account: null, effort: 'high' }, 'the effort stays when the model changes');
  assert.deepEqual(rungFromKey('or:a1:openai/gpt-6-luna', rungModels), { engine: 'codex', model: 'openai/gpt-6-luna', account: 'a1', effort: null });
  assert.equal(rungFromKey('or:a1:anthropic/claude-haiku-5.5', rungModels).engine, 'claude');
});

test('a ladder starts with the agents own model and grows by the next one not already on it', () => {
  const node = { engine: 'claude' as const, model: 'sonnet', account: null, ladder: [] as Rung[] };
  assert.deepEqual(nextRung(node, rungModels), { engine: 'claude', model: 'sonnet', account: null, effort: null });
  const one = { ...node, ladder: [nextRung(node, rungModels)] };
  assert.deepEqual(nextRung(one, rungModels), { engine: 'claude', model: 'opus', account: null, effort: null }, 'the first model not yet there');
  const all = { ...node, ladder: [...rungModels.claude.map((m) => ({ engine: 'claude' as const, model: m, account: null, effort: null })), ...rungModels.codex.map((m) => ({ engine: 'codex' as const, model: m, account: null, effort: null }))] };
  assert.equal(nextRung(all, rungModels).account, 'a1', 'then the OpenRouter ones, never a direct-call model');
  assert.ok(![...Array(3)].some((_, i) => nextRung({ ...all, ladder: [...all.ladder, ...rungModels.openrouter.slice(0, i).map((m) => ({ engine: m.driver, model: m.id, account: m.account, effort: null }))] }, rungModels).model === 'vendor/think'));
});

test('turning routing on gives an agent a ladder to start with, and turning it off keeps the ladder', () => {
  const base = { ...newNode(emptyFlow('f', 'F'), def('builder'), models, at), engine: 'claude' as const, model: 'sonnet' };
  assert.deepEqual(withRoutingChange(base, 'ladder', rungModels), { routing: 'ladder', ladder: [{ engine: 'claude', model: 'sonnet', account: null, effort: null }] });
  assert.deepEqual(withRoutingChange({ ...base, ladder: [{ engine: 'claude', model: 'haiku', account: null, effort: null }] }, 'auto', rungModels), { routing: 'auto' }, 'an existing ladder is kept');
  assert.deepEqual(withRoutingChange(base, 'pinned', rungModels), { routing: 'pinned' });
  assert.equal(ladderSummary({ routing: 'auto', ladder: [{ engine: 'claude', model: 'a', account: null, effort: null }] }), '1 rung, router picks');
  assert.equal(ladderSummary({ routing: 'ladder', ladder: [] }), null);
  assert.equal(ladderSummary({ routing: 'pinned', ladder: [{ engine: 'claude', model: 'a', account: null, effort: null }] }), null);
});

test('a goal with a web address offers to read it', () => {
  assert.equal(hasLink('Build it like https://example.com/docs please'), true);
  assert.equal(hasLink('see http://a.test'), true);
  assert.equal(hasLink('no link, only example.com and the word http'), false);
  assert.equal(hasLink('https://'), false);
});

test('every OpenRouter account has a meter, and the run share shows only on the ones its agents use', () => {
  const key = (id: string, name: string, remaining: number | null) => ({ id, name, spent: 5, daily: 0.5, monthly: 3, limit: remaining === null ? null : 50, remaining, error: null, fetchedAt: 1 });
  const usage = { claude: {} as never, codex: {} as never, openrouter: [key('a1', 'Main', 40), key('z9', 'Spare', null)] };
  const flow = { ...emptyFlow('f', 'F'), nodes: [{ ...newNode(emptyFlow('f', 'F'), def('builder'), models, at), account: 'a1' }] };
  const g = orGauges(flow, null, usage, { ...DEFAULT_LIMITS });
  assert.equal(g.length, 2, 'an account no agent uses still has a meter');
  assert.deepEqual([g[0]!.label, g[0]!.text, g[0]!.used, g[0]!.runUsd, g[0]!.tone], ['OpenRouter · Main', '$40.00 left', 20, null, 'ok']);
  assert.equal(orGauges(emptyFlow('f', 'F'), null, usage, { ...DEFAULT_LIMITS }).length, 2, 'a flow with no API agent shows them too');
  assert.deepEqual(orGauges(flow, null, { claude: {} as never, codex: {} as never }, { ...DEFAULT_LIMITS }), [], 'an older server says nothing');
});

test('an agent can be moved between your plan and each account, and lands on that places first model', () => {
  const node = { ...newNode(emptyFlow('f', 'F'), def('builder'), rungModels, at), engine: 'claude' as const, model: 'sonnet', account: null as string | null, direct: false };
  const here = providersFor(node, rungModels);
  assert.deepEqual(here.map((p) => p.key), ['plan', 'or:a1', 'or:z9']);
  assert.deepEqual(here.map((p) => p.label), ['My Claude / Codex plan', 'OpenRouter · Main', 'API · Plain (direct call, no tools)']);
  assert.equal(providerOf(node), 'plan');
  assert.deepEqual(providerPatch('or:a1', node, rungModels), { model: 'anthropic/claude-haiku-5.5', account: 'a1', direct: false, engine: 'claude' });
  assert.deepEqual(providerPatch('or:z9', node, rungModels), { model: 'vendor/think', account: 'z9', direct: true });
  assert.deepEqual(providerPatch('plan', { engine: 'codex' }, rungModels), { account: null, direct: false, model: 'gpt-6-luna' });
  assert.deepEqual(providerPatch('plan', node, rungModels), { account: null, direct: false, model: 'opus' });
  const gone = { ...node, account: 'dead' };
  assert.equal(providerOf(gone), 'or:dead');
  assert.equal(providersFor(gone, rungModels).at(-1)!.label, 'An account that is no longer available', 'an agent on a removed account still shows where it is');
  assert.deepEqual(providersFor(node, { ...rungModels, openrouter: [] }).map((p) => p.key), ['plan'], 'with no accounts there is only the plan');
});

test('the questions a run waits on come with their choices, without a number in front, and old ones are plain text', () => {
  assert.equal(unnumbered('1. Which database?'), 'Which database?');
  assert.equal(unnumbered('  12) Twelve'), 'Twelve');
  assert.equal(unnumbered('2024 plans?'), '2024 plans?', 'a year is not a number');
  const withChoices = { questions: ['1. A?', 'B?'], details: [{ text: '1. A?', recommended: 'x', options: ['x', 'y'], multiple: false }, { text: 'B?', recommended: '', options: [], multiple: false }] };
  const q = waitingQuestions(withChoices);
  assert.deepEqual(q.map((x) => [x.text, x.options?.length]), [['A?', 2], ['B?', 0]]);
  const old = waitingQuestions({ questions: ['3. Old one?'] });
  assert.deepEqual(old, [{ text: 'Old one?', recommended: '', options: [], multiple: false }], 'a pause from before there were choices is a text box');
  assert.equal(waitingQuestions({ questions: ['A?'], details: [] }).length, 1, 'details that do not match are ignored');
});

test('running a run again keeps its flow, brief and project, and fetches pages only if it had some', () => {
  const flow = { ...emptyFlow('f', 'F'), nodes: [newNode(emptyFlow('f', 'F'), def('builder'), models, at)] };
  const run = { flow, goal: 'Do X, see https://a.test', project: '/p', brief: 'The grilled brief', references: [{ url: 'https://a.test', text: 't', credits: 1 }], docs: [{ url: 'https://docs.test', text: 'd', credits: 10 }] } as unknown as Run;
  const req = rerunRequest(run);
  assert.deepEqual([req.goal, req.project, req.brief, req.skipGrill, req.readLinks, req.docs], ['Do X, see https://a.test', '/p', 'The grilled brief', true, true, ['https://docs.test']]);
  assert.equal(req.flow, flow, 'the agents and settings it ran with');
  const plain = rerunRequest({ ...run, brief: '', references: [], docs: undefined } as unknown as Run);
  assert.deepEqual([plain.brief, plain.readLinks, plain.docs], [undefined, false, []]);
});

test('a run waiting for a usage limit says when it carries on, by itself, and is not shown as needing you', () => {
  const now = Math.floor(Date.now() / 1000);
  const f = emptyFlow('f', 'F');
  const flow = { ...f, nodes: [{ ...newNode(f, def('builder'), models, at), id: 'n3', name: 'Architect' }] };
  const run = { flow, status: { state: 'paused', pause: { kind: 'limit', node: 'n3', message: "You've hit your session limit · resets 6pm (Europe/London)", resumeAt: now + 20 * 60 } }, retriesUsed: 0 } as unknown as Run;
  const info = pauseInfo(run)!;
  assert.equal(info.title, 'Architect hit a usage limit');
  assert.match(info.detail, /carries on by itself at .* \(in 20 minutes\)/);
  assert.match(info.detail, /do not need to do anything/);
  assert.deepEqual([info.primary!.label, info.primary!.action], ['Try now', 'resume']);
  assert.deepEqual(statusLabel(run.status), { text: 'Waiting', tone: 'warn' });
  assert.equal(untilLabel(now + 30, now), 'a moment');
  assert.equal(untilLabel(now + 3600, now), '1 hour');
  assert.equal(untilLabel(now + 3 * 3600 + 5 * 60, now), '3 hours 5 minutes');
  assert.equal(untilLabel(now + 2 * 86400, now), '2 days');
  assert.equal(untilLabel(now - 5, now), 'a moment');
  assert.ok(!resumeLabel(now + 600, now).includes(','), 'today is just a time');
  assert.ok(resumeLabel(now + 3 * 86400, now).includes(','), 'another day names the day');
});

test('the palette offers only the roles that are on offer, and the hidden ones still resolve for flows that use them', () => {
  const mk = (id: string, hidden?: boolean) => ({ ...def('builder'), id: id as Role, label: id, core: !hidden, hidden });
  const roles = [mk('builder'), mk('judge'), mk('architect', true), mk('reporter', true)];
  assert.deepEqual(offered(roles).map((r) => r.id), ['builder', 'judge']);
  setCatalog(roles);
  assert.equal(roleDef('architect')?.label, 'architect', 'a saved flow with an architect still finds its role');
  setCatalog([]);
});
