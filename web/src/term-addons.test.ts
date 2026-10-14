import assert from 'node:assert/strict';
import { test } from 'node:test';
import { linkAction, progressView } from './term-addons.ts';

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
