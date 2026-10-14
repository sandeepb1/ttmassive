import assert from 'node:assert/strict';
import { test } from 'node:test';
import { counter, parseFound, parts, stepTo, upLabel } from './search-model.ts';

test('a reply is read field by field', () => {
  const f = parseFound({
    total: 3,
    capped: true,
    hits: [{ n: 1, up: 0, text: 'a needle', at: 2, len: 6 }, { n: 'x' }, null, { n: 2, up: 4, text: 'b', at: 0, len: 1 }],
  });
  assert.equal(f.total, 3);
  assert.equal(f.capped, true);
  assert.deepEqual(f.hits.map((h) => h.n), [1, 2]); // the malformed entries are dropped
});

test('anything else is no result', () => {
  for (const raw of [null, undefined, 4, 'x', []]) assert.deepEqual(parseFound(raw), { total: 0, capped: false, hits: [] });
  assert.equal(parseFound({ hits: [{ n: 1, up: 0, text: 't', at: 0, len: 1 }] }).total, 1); // falls back to the list
});

test('stepping goes older or newer and stops at the ends', () => {
  assert.equal(stepTo(null, 'older', 5), 1);
  assert.equal(stepTo(null, 'newer', 5), 1);
  assert.equal(stepTo(1, 'older', 5), 2);
  assert.equal(stepTo(5, 'older', 5), 5);
  assert.equal(stepTo(3, 'newer', 5), 2);
  assert.equal(stepTo(1, 'newer', 5), 1);
  assert.equal(stepTo(null, 'older', 0), null);
});

test('the counter reads naturally', () => {
  assert.equal(counter(null, 0, false), 'No matches');
  assert.equal(counter(null, 1, false), '1 match');
  assert.equal(counter(null, 12, false), '12 matches');
  assert.equal(counter(3, 12, false), '3 of 12');
  assert.equal(counter(3, 1000, true), '3 of 1000+');
});

test('how far up a line is', () => {
  assert.equal(upLabel(0), 'last line');
  assert.equal(upLabel(1), '1 line up');
  assert.equal(upLabel(120), '120 lines up');
});

test('a hit splits around its match, by characters not bytes', () => {
  assert.deepEqual(parts({ n: 1, up: 0, text: 'a needle here', at: 2, len: 6 }), ['a ', 'needle', ' here']);
  assert.deepEqual(parts({ n: 1, up: 0, text: '😀x', at: 1, len: 1 }), ['😀', 'x', '']);
  assert.deepEqual(parts({ n: 1, up: 0, text: '…abc', at: 1, len: 9 }), ['…', 'abc', '']);
});
