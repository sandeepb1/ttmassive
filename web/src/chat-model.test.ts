import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  applyChat,
  chipFor,
  currentEffort,
  chooseView,
  buildTarget,
  lastUserMessage,
  noteLeavingBuild,
  pasteHint,
  pasteLabel,
  modeChipText,
  modeTone,
  stepEffort,
  isRecorded,
  levelKey,
  modelChip,
  newStore,
  pendingQuestion,
  prettyModel,
  setOverride,
  type ChatItem,
  type ChatResponse,
} from './chat-model.ts';

const user = (id: string, seq: number, text = id): ChatItem => ({ id, seq, ts: null, kind: 'user', text, images: 0 });
const tool = (id: string, seq: number, status: 'running' | 'ok'): ChatItem => ({
  id,
  seq,
  ts: null,
  kind: 'tool',
  tool: 'Bash',
  summary: 'run',
  input: null,
  output: null,
  status,
  diff: null,
});
const question = (id: string, seq: number, state: 'pending' | 'answered'): ChatItem => ({
  id,
  seq,
  ts: null,
  kind: 'question',
  questions: [],
  state,
  answers: [],
});

const resp = (items: ChatItem[], over: Partial<ChatResponse> = {}): ChatResponse => ({
  epoch: 1,
  seq: items.reduce((m, i) => Math.max(m, i.seq), 0),
  reset: false,
  items,
  model: null,
  effort: null,
  model_label: null,
  mode: null,
  modes: [],
  yolo: false,
  effort_live: null,
  suggestion: null,
  eligible: true,
  agent: 'claude',
  ready: true,
  screen: null,
  ...over,
});

test('the first response replaces everything, and later ones append', () => {
  const s = newStore();
  assert.deepEqual(applyChat(s, resp([user('a', 1), user('b', 2)], { reset: true })), { reset: true, touched: ['a', 'b'] });
  assert.deepEqual(applyChat(s, resp([user('c', 3)])), { reset: false, touched: ['c'] });
  assert.deepEqual(s.items.map((i) => i.id), ['a', 'b', 'c']);
  assert.equal(s.seq, 3);
});

test('a changed item is replaced where it stands, not moved to the end', () => {
  const s = newStore();
  applyChat(s, resp([tool('t1', 1, 'running'), user('u1', 2)], { reset: true }));
  const r = applyChat(s, resp([tool('t1', 3, 'ok')]));
  assert.deepEqual(r.touched, ['t1']);
  assert.deepEqual(s.items.map((i) => i.id), ['t1', 'u1']);
  assert.equal(s.items[0].kind === 'tool' && s.items[0].status, 'ok');
});

test('a different epoch starts over even if the server did not say reset', () => {
  const s = newStore();
  applyChat(s, resp([user('a', 1), user('b', 2)], { reset: true }));
  const r = applyChat(s, resp([user('x', 1)], { epoch: 2 }));
  assert.equal(r.reset, true);
  assert.deepEqual(s.items.map((i) => i.id), ['x']);
});

test('model and effort stick until the server reports new ones', () => {
  const s = newStore();
  applyChat(s, resp([], { reset: true, model: 'claude-sonnet-5-5', effort: 'medium' }));
  applyChat(s, resp([], { model: null, effort: null }));
  assert.equal(modelChip(s.model, s.effort), 'Sonnet 5.5 · medium');
  applyChat(s, resp([], { model: 'claude-opus-5-5', effort: 'high' }));
  assert.equal(modelChip(s.model, s.effort), 'Opus 5.5 · high');
});

test('model names read naturally', () => {
  assert.equal(prettyModel('claude-sonnet-5-5'), 'Sonnet 5.5');
  assert.equal(prettyModel('claude-opus-5'), 'Opus 5');
  assert.equal(prettyModel('claude-haiku-4-5-20251001'), 'Haiku 4.5');
  assert.equal(prettyModel('gpt-6.1-sol'), 'GPT-6.1-Sol');
  assert.equal(prettyModel('something-else'), 'something-else');
  assert.equal(prettyModel(null), '');
  assert.equal(modelChip(null, 'high'), 'high');
  assert.equal(modelChip(null, null), '');
});

test('the pending question is the newest unanswered one', () => {
  const items = [question('q1', 1, 'answered'), user('u', 2), question('q2', 3, 'pending')];
  assert.equal(pendingQuestion(items)?.id, 'q2');
  assert.equal(pendingQuestion([question('q1', 1, 'answered')]), undefined);
  assert.equal(pendingQuestion([]), undefined);
});

test('a saved view wins, but only agent sessions can be chat', () => {
  assert.equal(chooseView(null, true), 'chat');
  assert.equal(chooseView('term', true), 'term');
  assert.equal(chooseView('chat', true), 'chat');
  assert.equal(chooseView('chat', false), 'term');
  assert.equal(chooseView('garbage', true), 'chat');
});

test('a sent message counts as recorded once a newer matching user item appears', () => {
  const items = [user('old', 5, 'hello'), user('new', 9, ' hello ')];
  assert.equal(isRecorded(items, 'hello', 5), true); // the newer one, ignoring surrounding spaces
  assert.equal(isRecorded(items, 'hello', 9), false); // nothing newer than that
  assert.equal(isRecorded(items, 'other', 0), false);
});

test('a model or effort you just chose shows until the log reports a different one', () => {
  const s = newStore();
  applyChat(s, resp([], { reset: true, model: 'claude-sonnet-5-5', effort: 'medium' }));
  assert.equal(chipFor(s), 'Sonnet 5.5 · medium');
  setOverride(s, { model: 'Haiku 5.5', effort: 'high' });
  assert.equal(chipFor(s), 'Haiku 5.5 · high');
  // Polls that still carry the old values keep the override.
  applyChat(s, resp([], { model: 'claude-sonnet-5-5', effort: 'medium' }));
  assert.equal(chipFor(s), 'Haiku 5.5 · high');
  // The agent's next reply reports the new model: the log has caught up and is believed again.
  applyChat(s, resp([], { model: 'claude-haiku-5-5', effort: 'high' }));
  assert.equal(chipFor(s), 'Haiku 5.5 · high');
  assert.equal(s.modelOverride, null);
  assert.equal(s.effortOverride, null);
});

test('changing only the effort leaves the model coming from the log', () => {
  const s = newStore();
  applyChat(s, resp([], { reset: true, model: 'claude-opus-5-5', effort: 'medium' }));
  setOverride(s, { effort: 'low' });
  assert.equal(chipFor(s), 'Opus 5.5 · low');
  applyChat(s, resp([], { model: 'claude-sonnet-5-5', effort: 'medium' })); // the model changed elsewhere
  assert.equal(chipFor(s), 'Sonnet 5.5 · low');
});

test('a model named in a /model confirmation shows until a reply reports the real one', () => {
  const s = newStore();
  applyChat(s, resp([], { reset: true, model: 'claude-sonnet-5-5', effort: 'medium' }));
  applyChat(s, resp([], { model_label: 'Haiku 5.5', effort: 'high' }));
  assert.equal(chipFor(s), 'Haiku 5.5 · high');
  applyChat(s, resp([], { model: 'claude-haiku-5-5', model_label: null, effort: 'high' }));
  assert.equal(chipFor(s), 'Haiku 5.5 · high');
  assert.equal(s.modelLabel, null);
});

test('Codex level names become the keys the chip and log use', () => {
  assert.equal(levelKey('Low'), 'low');
  assert.equal(levelKey('Medium'), 'medium');
  assert.equal(levelKey('Extra high'), 'xhigh');
  assert.equal(levelKey(' HIGH '), 'high');
});

test('a Codex change shows its model and level until the log catches up', () => {
  const s = newStore();
  applyChat(s, resp([], { reset: true, model: 'gpt-6.1-sol', effort: 'medium' }));
  assert.equal(chipFor(s), 'GPT-6.1-Sol · medium');
  setOverride(s, { model: 'GPT-6-Luna', effort: levelKey('Extra high') });
  assert.equal(chipFor(s), 'GPT-6-Luna · xhigh');
  applyChat(s, resp([], { model: 'gpt-6-luna', effort: 'xhigh' }));
  assert.equal(s.modelOverride, null);
  assert.equal(chipFor(s), 'GPT-6-Luna · xhigh');
});

type ModeIdLike = 'default' | 'auto' | 'manual' | 'accept_edits' | 'plan' | 'bypass';

const CLAUDE_MODES = [
  { id: 'plan', label: 'Plan', hint: '' },
  { id: 'accept_edits', label: 'Accept edits', hint: '' },
  { id: 'manual', label: 'Manual', hint: '' },
  { id: 'auto', label: 'Auto', hint: '' },
] as const;

test('Build goes back to the mode you were in before plan', () => {
  const s = newStore();
  const at = (mode: ModeIdLike) => applyChat(s, resp([], { mode, modes: [...CLAUDE_MODES] }));
  at('manual');
  at('manual');
  assert.equal(buildTarget(s)?.id, 'manual');
  at('plan');
  at('plan');
  assert.equal(buildTarget(s)?.id, 'manual', 'plan must not overwrite the mode to go back to');
  at('auto');
  at('auto');
  assert.equal(buildTarget(s)?.id, 'auto');
});

test('modes passed through on the way to plan are not taken for where you were building', () => {
  const s = newStore();
  const at = (mode: ModeIdLike) => applyChat(s, resp([], { mode, modes: [...CLAUDE_MODES] }));
  at('auto');
  at('auto');
  // Auto to Plan cycles through Manual and Accept edits; a poll can catch one of them once.
  at('manual');
  at('plan');
  at('plan');
  assert.equal(buildTarget(s)?.id, 'auto');
});

test('tapping Plan records the mode you are leaving, even before a second poll', () => {
  const s = newStore();
  applyChat(s, resp([], { reset: true, mode: 'auto', modes: [...CLAUDE_MODES] }));
  assert.equal(buildTarget(s)?.id, 'accept_edits', 'one sighting is not trusted yet');
  noteLeavingBuild(s, s.mode);
  assert.equal(buildTarget(s)?.id, 'auto');
  noteLeavingBuild(s, 'plan'); // leaving plan is not leaving a build mode
  noteLeavingBuild(s, null);
  assert.equal(buildTarget(s)?.id, 'auto');
});

test('with no history, Build defaults to accept edits for Claude and default for Codex', () => {
  const claude = newStore();
  applyChat(claude, resp([], { reset: true, mode: 'plan', modes: [...CLAUDE_MODES] }));
  assert.equal(buildTarget(claude)?.id, 'accept_edits');
  const codex = newStore();
  const modes = [
    { id: 'default', label: 'Default', hint: '' },
    { id: 'plan', label: 'Plan', hint: '' },
  ] as const;
  applyChat(codex, resp([], { reset: true, mode: 'plan', modes: [...modes] }));
  assert.equal(buildTarget(codex)?.id, 'default');
  assert.equal(buildTarget(newStore()), null, 'no modes, no build target');
});

test('a remembered build mode that is no longer on offer falls back to one that is', () => {
  const s = newStore();
  const withBypass = [{ id: 'bypass', label: 'Bypass', hint: '' }, ...CLAUDE_MODES] as const;
  applyChat(s, resp([], { reset: true, mode: 'bypass', modes: [...withBypass] }));
  applyChat(s, resp([], { mode: 'bypass', modes: [...withBypass] }));
  applyChat(s, resp([], { mode: 'plan', modes: [...CLAUDE_MODES] })); // bypass is gone from the list
  assert.equal(buildTarget(s)?.id, 'accept_edits');
});

test('the mode chip names the mode and flags skipped permissions', () => {
  const s = newStore();
  assert.equal(modeChipText(s), '');
  applyChat(s, resp([], { reset: true, mode: 'accept_edits', modes: [...CLAUDE_MODES] }));
  assert.equal(modeChipText(s), 'Accept edits');
  applyChat(s, resp([], { mode: 'accept_edits', modes: [...CLAUDE_MODES], yolo: true }));
  assert.equal(modeChipText(s), '⚡ Accept edits');
});

test('effort steps one notch and stops at the ends', () => {
  assert.equal(stepEffort('medium', true), 'high');
  assert.equal(stepEffort('medium', false), 'low');
  assert.equal(stepEffort('low', false), null);
  assert.equal(stepEffort('max', true), null);
  assert.equal(stepEffort('xhigh', true), 'max');
  assert.equal(stepEffort(null, true), null);
  assert.equal(stepEffort('ludicrous', true), null);
});

test('the last message you sent is found for editing', () => {
  assert.equal(lastUserMessage([]), null);
  assert.equal(lastUserMessage([user('a', 1, 'first'), user('b', 2, 'second'), tool('t', 3, 'ok')]), 'second');
  assert.equal(lastUserMessage([user('a', 1, '   ')]), null);
});

test('the reasoning on the agent screen is believed over a stale log', () => {
  const s = newStore();
  applyChat(s, resp([], { reset: true, model: 'claude-sonnet-5-5', effort: 'medium' }));
  assert.equal(currentEffort(s), 'medium');
  // Changed in the terminal: the screen knows at once, the log only after the next reply.
  applyChat(s, resp([], { model: 'claude-sonnet-5-5', effort: 'medium', effort_live: 'high' }));
  assert.equal(currentEffort(s), 'high');
  assert.equal(chipFor(s), 'Sonnet 5.5 · high');
  // A change you just made through the app wins until the screen shows the same.
  setOverride(s, { effort: 'low' });
  assert.equal(currentEffort(s), 'low');
  // With no log yet (a new session), the screen is all there is.
  const fresh = newStore();
  applyChat(fresh, resp([], { reset: true, effort_live: 'medium' }));
  assert.equal(currentEffort(fresh), 'medium');
  assert.equal(currentEffort(newStore()), null);
});

test('applyChat keeps the suggested reply and clears it when it goes away', () => {
  const s = newStore();
  applyChat(s, resp([], { suggestion: 'go ahead with step 2' }));
  assert.equal(s.suggestion, 'go ahead with step 2');
  applyChat(s, resp([], { suggestion: null }));
  assert.equal(s.suggestion, null);
});

test('a paste is labelled by its length and hinted by its first line', () => {
  assert.equal(pasteLabel('one'), 'Pasted text · 1 line');
  assert.equal(pasteLabel('a\nb\nc'), 'Pasted text · 3 lines');
  assert.equal(pasteLabel(''), 'Pasted text · 0 lines');
  assert.equal(pasteHint('\n\n  ~/code main*  \n❯ ls'), '~/code main*');
  assert.equal(pasteHint(''), '');
  assert.equal(pasteHint('x'.repeat(100), 20), `${'x'.repeat(19)}…`);
});

test('the mode chip is tinted by how much the agent may do on its own', () => {
  assert.equal(modeTone('default', false), 'neutral');
  assert.equal(modeTone(null, false), 'neutral');
  assert.equal(modeTone('plan', false), 'accent');
  assert.equal(modeTone('accept_edits', false), 'ok');
  assert.equal(modeTone('auto', false), 'warn');
  assert.equal(modeTone('bypass', false), 'danger');
  assert.equal(modeTone('plan', true), 'danger');
});
