import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  accountState,
  accountsFor,
  canSwitchTo,
  compact,
  dashboardReachable,
  pct,
  providerOf,
  recentModels,
  requestAge,
  statusKind,
  toMs,
  uptime,
  type TcStatus,
} from './teamclaude-model.ts';

const status: TcStatus = {
  accounts: [
    { name: 's@ndeep.me', provider: 'anthropic', priority: 0 },
    { name: 's@ndeep.me', provider: 'codex', priority: 0 },
    { name: 'claude@sndp.me', provider: 'anthropic', priority: 0 },
    { name: 'spare@sndp.me', provider: 'anthropic', priority: 1 },
  ],
  currentAccounts: { anthropic: 'claude@sndp.me', codex: 's@ndeep.me' },
};

test('agents map to the proxy providers', () => {
  assert.equal(providerOf('claude'), 'anthropic');
  assert.equal(providerOf('codex'), 'codex');
  assert.equal(providerOf('other'), null);
  assert.equal(providerOf(null), null);
});

test('accounts are listed for one provider, the active one first', () => {
  assert.deepEqual(accountsFor(status, 'anthropic').map((a) => a.name), ['claude@sndp.me', 's@ndeep.me', 'spare@sndp.me']);
  assert.deepEqual(accountsFor(status, 'codex').map((a) => a.name), ['s@ndeep.me']);
  assert.deepEqual(accountsFor(undefined, 'codex'), []);
});

test('only an account the proxy can reach by name is offered', () => {
  assert.equal(canSwitchTo(status, 'anthropic', 's@ndeep.me'), true, 'the first account of that name is Anthropic’s');
  assert.equal(canSwitchTo(status, 'anthropic', 'claude@sndp.me'), false, 'already in use');
  assert.equal(canSwitchTo(status, 'codex', 's@ndeep.me'), false, 'the proxy would pick Anthropic’s');
  assert.equal(canSwitchTo(status, 'codex', 'nobody'), false);
});

test('an account’s state', () => {
  const now = 1_000_000;
  assert.equal(accountState({ name: 'a', status: 'active' }, true, now), 'active');
  assert.equal(accountState({ name: 'a', status: 'active' }, false, now), 'ready');
  assert.equal(accountState({ name: 'a', disabled: true }, true, now), 'disabled');
  assert.equal(accountState({ name: 'a', rateLimitedUntil: now + 5000 }, true, now), 'limited');
  assert.equal(accountState({ name: 'a', rateLimitedUntil: now - 5000 }, true, now), 'active', 'a limit that has passed');
  assert.equal(accountState({ name: 'a', pausedUntil: new Date(now + 5000).toISOString() }, false, now), 'paused');
  assert.equal(accountState({ name: 'a', status: 'credential_rejected' }, false, now), 'unavailable');
});

test('times from the proxy', () => {
  assert.equal(toMs(1791994200000), 1791994200000);
  assert.equal(toMs('2026-10-14T14:14:34.370Z'), Date.parse('2026-10-14T14:14:34.370Z'));
  assert.equal(toMs('soon'), null);
  assert.equal(toMs(null), null);
  assert.equal(toMs(undefined), null);
});

test('percentages and counts', () => {
  assert.equal(pct(0.82), 82);
  assert.equal(pct(1.3), 100);
  assert.equal(pct(null), null);
  assert.equal(pct(undefined), null);
  assert.equal(compact(4054), '4.1k');
  assert.equal(compact(2165019), '2.2M');
  assert.equal(compact(12000), '12k');
  assert.equal(compact(42), '42');
  assert.equal(compact(undefined), '–');
});

test('uptime', () => {
  assert.equal(uptime(456517), '5d 6h');
  assert.equal(uptime(3 * 3600 + 12 * 60), '3h 12m');
  assert.equal(uptime(480), '8m');
  assert.equal(uptime(42), '42s');
  assert.equal(uptime(undefined), '');
  assert.equal(uptime(-1), '');
});

test('recent models, newest first, without date suffixes', () => {
  const a = {
    name: 'a',
    usage: { recentModels: { 'claude-haiku-4-5-20251001': 1791550064217, 'claude-sonnet-5-5': 1791987274370, 'claude-opus-5-5': 1791986601964 } },
  };
  assert.deepEqual(recentModels(a), ['claude-sonnet-5-5', 'claude-opus-5-5', 'claude-haiku-4-5']);
  assert.deepEqual(recentModels(a, 1), ['claude-sonnet-5-5']);
  assert.deepEqual(recentModels({ name: 'b' }), []);
});

test('request colours and ages', () => {
  assert.equal(statusKind({ status: 200, error: false }), 'ok');
  assert.equal(statusKind({ status: 404, error: false }), 'warn');
  assert.equal(statusKind({ status: 429, error: false }), 'err');
  assert.equal(statusKind({ status: 529, error: false }), 'err');
  assert.equal(statusKind({ status: null, error: false }), 'warn', 'still in flight');
  assert.equal(statusKind({ status: null, error: true }), 'err');
  assert.equal(requestAge(100, 112), '12s');
  assert.equal(requestAge(100, 340), '4m');
  assert.equal(requestAge(100, 100 + 3 * 3600), '3h');
  assert.equal(requestAge(100, 100 + 2 * 86_400), '2d');
  assert.equal(requestAge(200, 100), '0s', 'a clock a little behind');
});

test('the dashboard link needs the page and the proxy on the same machine', () => {
  const reply = { detected: true, agent: 'claude', error: null, dashboard: 'http://127.0.0.1:3456/teamclaude/dashboard', loopback: true } as const;
  assert.equal(dashboardReachable(reply, 'localhost'), true);
  assert.equal(dashboardReachable(reply, '127.0.0.1'), true);
  assert.equal(dashboardReachable(reply, 'ttm.example.com'), false);
  assert.equal(dashboardReachable({ ...reply, loopback: false }, 'localhost'), false);
  assert.equal(dashboardReachable({ ...reply, dashboard: undefined }, 'localhost'), false);
});
