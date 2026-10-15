import assert from 'node:assert/strict';
import { test } from 'node:test';
import { bellRings, cleanTitle, linkAction, mayFocusTerminal, progressView } from './term-addons.ts';

test('progress: hidden when off', () => {
  assert.equal(progressView({ state: 0, value: 0 }), null);
});

test('progress: normal clamps and rounds', () => {
  assert.deepEqual(progressView({ state: 1, value: 42.4 }), { kind: 'normal', pct: 42 });
  assert.deepEqual(progressView({ state: 1, value: 250 }), { kind: 'normal', pct: 100 });
  assert.deepEqual(progressView({ state: 1, value: -5 }), { kind: 'normal', pct: 0 });
});

test('progress: error and paused stay visible without a value', () => {
  assert.deepEqual(progressView({ state: 2, value: 0 }), { kind: 'error', pct: 100 });
  assert.deepEqual(progressView({ state: 2, value: 30 }), { kind: 'error', pct: 30 });
  assert.deepEqual(progressView({ state: 4, value: 0 }), { kind: 'paused', pct: 100 });
});

test('progress: indeterminate has no length', () => {
  assert.deepEqual(progressView({ state: 3, value: 0 }), { kind: 'indeterminate', pct: null });
});

test('links: ordinary labels open', () => {
  assert.equal(linkAction('PR #12', 'https://github.com/a/b/pull/12'), 'open');
  assert.equal(linkAction('here', 'http://localhost:3000/'), 'open');
});

test('links: matching address label opens', () => {
  assert.equal(linkAction('github.com', 'https://github.com/a/b'), 'open');
  assert.equal(linkAction('https://github.com/a/b', 'https://GitHub.com/a/b'), 'open');
});

test('links: address label over another host asks first', () => {
  assert.equal(linkAction('github.com', 'https://evil.example/login'), 'confirm');
  assert.equal(linkAction('https://github.com/x', 'https://github.com.evil.example/x'), 'confirm');
});

test('links: only http(s) and mailto are allowed', () => {
  assert.equal(linkAction('x', 'javascript:alert(1)'), 'block');
  assert.equal(linkAction('x', 'file:///etc/passwd'), 'block');
  assert.equal(linkAction('x', 'not a url'), 'block');
  assert.equal(linkAction('me@example.com', 'mailto:me@example.com'), 'open');
});

test('titles lose spinner glyphs, control characters and extra space, and are cut short', () => {
  assert.equal(cleanTitle('✳ Refactor auth'), 'Refactor auth');
  assert.equal(cleanTitle('⠂  Fix\tthe   build\x07'), 'Fix the build');
  assert.equal(cleanTitle('~/code — zsh'), 'code — zsh');
  assert.equal(cleanTitle('✻'), '');
  assert.equal(cleanTitle('a'.repeat(100), 10), 'aaaaaaaaa…');
});

test('a bell rings for an unfocused or hidden pane, at most once per gap', () => {
  const base = { now: 100_000, last: 0, focused: false, hidden: false, gapMs: 10_000 };
  assert.equal(bellRings(base), true);
  assert.equal(bellRings({ ...base, focused: true }), false);
  assert.equal(bellRings({ ...base, focused: true, hidden: true }), true);
  assert.equal(bellRings({ ...base, last: 95_000 }), false);
  assert.equal(bellRings({ ...base, last: 90_000 }), true);
});

test('a mouse or keyboard user always gets the terminal focused', () => {
  assert.equal(mayFocusTerminal({ coarsePointer: false, keyboardUp: false }), true);
  assert.equal(mayFocusTerminal({ coarsePointer: false, keyboardUp: true }), true);
});

test('on a touch screen the keyboard is not raised by the app, only kept', () => {
  assert.equal(mayFocusTerminal({ coarsePointer: true, keyboardUp: false }), false);
  assert.equal(mayFocusTerminal({ coarsePointer: true, keyboardUp: true }), true);
});
