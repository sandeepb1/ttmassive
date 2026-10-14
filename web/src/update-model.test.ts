import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildOf, isStale } from './update-model.ts';

const page = (js: string) => `<!doctype html><html><head><script type="module" crossorigin src="${js}"></script><link rel="stylesheet" href="/assets/index-x.css"></head></html>`;

test('the build is the hashed entry script', () => {
  assert.equal(buildOf(page('/assets/index-abc123.js')), '/assets/index-abc123.js');
});

test('attribute order does not matter', () => {
  assert.equal(buildOf('<script src="/assets/index-q.js" type="module"></script>'), '/assets/index-q.js');
});

test('a dev page has no build', () => {
  assert.equal(buildOf('<script type="module" src="/src/main.ts"></script>'), null);
});

test('stale only when both builds are known and differ', () => {
  assert.equal(isStale('/assets/a.js', '/assets/b.js'), true);
  assert.equal(isStale('/assets/a.js', '/assets/a.js'), false);
  assert.equal(isStale('/assets/a.js', null), false); // offline: do not guess
  assert.equal(isStale(null, '/assets/b.js'), false);
});
