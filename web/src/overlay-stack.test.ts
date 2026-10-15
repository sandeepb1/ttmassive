import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createOverlayStack } from './overlay-stack.ts';

/** A history that records what was asked of it. `back()` only takes effect when the test says the browser did it. */
function fake() {
  const log: string[] = [];
  return { log, history: { pushState: () => void log.push('push'), back: () => void log.push('back') } };
}

const settle = () => new Promise<void>((r) => queueMicrotask(r));

test('one history entry however many overlays are open', () => {
  const { log, history } = fake();
  const s = createOverlayStack(history);
  s.push(() => {});
  s.push(() => {});
  assert.deepEqual(log, ['push']);
});

test('Back closes the top overlay and keeps the entry while others remain', () => {
  const { log, history } = fake();
  const s = createOverlayStack(history);
  const closed: string[] = [];
  let releaseA = () => {};
  let releaseB = () => {};
  releaseA = s.push(() => (closed.push('a'), releaseA()));
  releaseB = s.push(() => (closed.push('b'), releaseB()));
  s.popped();
  assert.deepEqual(closed, ['b']);
  assert.deepEqual(log, ['push', 'push']); // the entry Back consumed is put back for 'a'
  assert.equal(s.depth, 1);
});

test('Back on the last overlay leaves no entry behind and does not go back again', () => {
  const { log, history } = fake();
  const s = createOverlayStack(history);
  let release = () => {};
  release = s.push(() => release());
  s.popped();
  assert.deepEqual(log, ['push']);
  assert.equal(s.depth, 0);
});

test('closing the last overlay by hand removes the entry', async () => {
  const { log, history } = fake();
  const s = createOverlayStack(history);
  const release = s.push(() => {});
  release();
  await settle();
  assert.deepEqual(log, ['push', 'back']);
});

test('a menu that closes as the sheet it launches opens keeps the one entry', async () => {
  const { log, history } = fake();
  const s = createOverlayStack(history);
  const releaseMenu = s.push(() => {});
  releaseMenu();
  s.push(() => {});
  await settle();
  assert.deepEqual(log, ['push']);
});

test('a push while our own back is still in flight waits for it', async () => {
  const { log, history } = fake();
  const s = createOverlayStack(history);
  const release = s.push(() => {});
  release();
  await settle(); // back is now pending
  s.push(() => {});
  assert.deepEqual(log, ['push', 'back']); // not pushed yet: the pending back would pop it
  s.popped(); // the browser finished going back
  assert.deepEqual(log, ['push', 'back', 'push']);
});

test('the browser going back with no overlay is not ours', () => {
  const { log, history } = fake();
  const s = createOverlayStack(history);
  s.popped();
  assert.deepEqual(log, []);
});

test('an overlay that ignores close is still dropped, so Back cannot stick on it', () => {
  const { history } = fake();
  const s = createOverlayStack(history);
  s.push(() => {});
  s.popped();
  assert.equal(s.depth, 0);
});

test('popped says whether the step was ours, so the page can tell overlay history from navigation', () => {
  const { history } = fake();
  const s = createOverlayStack(history);
  assert.equal(s.popped(), false); // nothing open: someone else's navigation
  let release = () => {};
  release = s.push(() => release());
  assert.equal(s.popped(), true); // the user's Back closed the overlay
});

test('our own back going away is reported as ours', async () => {
  const { history } = fake();
  const s = createOverlayStack(history);
  const release = s.push(() => {});
  release();
  await settle(); // back is in flight
  assert.equal(s.popped(), true);
});
