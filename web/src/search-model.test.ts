import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ago, counter, parseFound, parseTurns, parts, stepTo, stepTurn, turnCounter, upLabel, type Turn } from './search-model.ts';

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


const turn = (id: string, n: number | null): Turn => ({ id, ts: null, preview: id, snippet: id, n, text: n === null ? null : id, up: n });

test('a turns reply is read field by field', () => {
  const t = parseTurns({
    turns: [
      { id: 'a', ts: 100, preview: 'fix it', snippet: 'fix it', n: 1, text: '> fix it', up: 0 },
      { id: 'b', ts: null, preview: 'gone', snippet: 'gone', n: null, text: null, up: null },
      { id: 'c', preview: 'half', snippet: 'half', n: 3 }, // placed but no line to send back: not placed
      { preview: 'no id' },
      null,
    ],
  });
  assert.deepEqual(t.map((x) => [x.id, x.n]), [['a', 1], ['b', null], ['c', null]]);
  assert.equal(t[0]!.ts, 100);
  assert.deepEqual(parseTurns(null), []);
  assert.deepEqual(parseTurns({ turns: 'x' }), []);
});

test('stepping through messages skips the ones that are not on the terminal', () => {
  const t = [turn('new', 1), turn('lost', null), turn('old', 2)];
  assert.equal(stepTurn(t, null, 'older'), 0); // start at the newest
  assert.equal(stepTurn(t, 0, 'older'), 2); // past the one that is gone
  assert.equal(stepTurn(t, 2, 'older'), null); // the end
  assert.equal(stepTurn(t, 2, 'newer'), 0);
  assert.equal(stepTurn(t, 0, 'newer'), null);
  assert.equal(stepTurn(t, null, 'newer'), 2); // newer from nothing: the oldest
  assert.equal(stepTurn([turn('x', null)], null, 'older'), null);
  assert.equal(stepTurn([], null, 'older'), null);
});

test('the turn counter counts only the ones that can be jumped to', () => {
  const t = [turn('new', 1), turn('lost', null), turn('old', 2)];
  assert.equal(turnCounter(t, null), '3 messages');
  assert.equal(turnCounter(t, 0), '1 of 2');
  assert.equal(turnCounter(t, 2), '2 of 2');
  assert.equal(turnCounter([turn('a', 1)], null), '1 message');
  assert.equal(turnCounter([], null), '');
});

test('time ago', () => {
  assert.equal(ago(null, 1000), '');
  assert.equal(ago(990, 1000), 'just now');
  assert.equal(ago(1000 - 300, 1000), '5 min ago');
  assert.equal(ago(1000 - 7200, 1000), '2 h ago');
  assert.equal(ago(1000 - 3 * 86400, 1000), '3 d ago');
  assert.equal(ago(2000, 1000), 'just now'); // a clock a little ahead
});
