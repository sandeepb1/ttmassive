import assert from 'node:assert/strict';
import { test } from 'node:test';
import { arrived, checkedAgo, failureNews, headline, offerText, shouldOffer, type ReleaseInfo, type ServerUpdate } from './server-update-model.ts';

const rel = (version: string, title: string): ReleaseInfo => ({ version, title, notes: '', url: '', publishedAt: '' });
const upd = (releases: ReleaseInfo[]): ServerUpdate => ({
  current: '0.1.0',
  latest: releases[0]?.version ?? null,
  releases,
  checkedAt: null,
  error: null,
  canInstall: true,
  cannotInstallReason: null,
  installing: false,
  lastResult: null,
});

test('the headline is the title without the version', () => {
  assert.equal(headline(rel('0.3.0', 'v0.3.0 — Fix a sheet')), 'Fix a sheet');
  assert.equal(headline(rel('0.3.0', 'v0.3.0')), '');
});

test('the toast names the version and the headline', () => {
  assert.equal(offerText(upd([rel('0.3.0', 'v0.3.0 — Fix a sheet'), rel('0.2.0', 'v0.2.0 — x')])), 'ttm v0.3.0 — Fix a sheet');
  assert.equal(offerText(upd([rel('0.3.0', 'v0.3.0')])), 'ttm v0.3.0');
  assert.equal(offerText(upd([])), null);
});

test('a version is offered once', () => {
  const u = upd([rel('0.3.0', 'v0.3.0 — x')]);
  assert.equal(shouldOffer(u, null), true);
  assert.equal(shouldOffer(u, '0.3.0'), false);
  assert.equal(shouldOffer(upd([rel('0.4.0', 'v0.4.0 — y')]), '0.3.0'), true); // a newer one is news again
  assert.equal(shouldOffer(upd([]), null), false);
});

test('a failed update is news until it has been seen', () => {
  const last = { ok: false, from: '0.1.0', to: '0.2.0', at: 100 };
  assert.match(failureNews(last, 0) ?? '', /v0\.2\.0 did not start/);
  assert.equal(failureNews(last, 100), null);
  assert.equal(failureNews({ ...last, ok: true }, 0), null);
  assert.equal(failureNews(null, 0), null);
});

test('the restarted server has arrived when it reports the target version', () => {
  assert.equal(arrived(upd([]), '0.1.0'), true);
  assert.equal(arrived(upd([]), '0.2.0'), false);
});

test('how long ago, in words', () => {
  assert.equal(checkedAgo(null, 1000), 'not checked yet');
  assert.equal(checkedAgo(990, 1000), 'checked just now');
  assert.equal(checkedAgo(1000 - 5 * 60, 1000), 'checked 5 min ago');
  assert.equal(checkedAgo(1000 - 3 * 3600, 1000), 'checked 3 h ago');
});
