import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isOrchHash } from './entry.ts';

// A session name is 1 to 64 of letters, digits, - and _ (src/tmux.rs valid_session_id).
const isSessionName = (n: string) => /^[A-Za-z0-9_-]{1,64}$/.test(n);

test('the orchestrator addresses are never valid session names', () => {
  for (const h of ['#orch/', '#orch/r6ad0e7e20ffea8']) {
    assert.equal(isOrchHash(h), true, h);
    assert.equal(isSessionName(decodeURIComponent(h.slice(1))), false, `${h} must not read as a session`);
  }
});

test('a session called orch is a session, not the mode', () => {
  assert.equal(isOrchHash('#orch'), false);
  assert.equal(isOrchHash('#orchard'), false);
  assert.equal(isOrchHash('#main'), false);
  assert.equal(isOrchHash(''), false);
});
