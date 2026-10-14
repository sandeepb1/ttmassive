import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LineCarry, addSample, clampLines, flings, inputWhileScrolled, keepGoing, liveLabel, slow, velocity } from './touch-scroll.ts';

test('finger travel becomes whole lines and the remainder is kept', () => {
  const c = new LineCarry();
  assert.equal(c.add(10, 20), 0);
  assert.equal(c.add(10, 20), 1, 'two small moves add up to a line');
  assert.equal(c.add(45, 20), 2);
  assert.equal(c.add(5, 20), 0, '5px were left over, and 5 more is only half a line');
  assert.equal(c.add(10, 20), 1, 'but 10 more finishes it');
  assert.equal(c.add(-30, 20), -1, 'the other way');
  assert.equal(c.add(-30, 20), -2, 'and the half line left over carries too');
  c.reset();
  assert.equal(c.add(19, 20), 0);
});

test('nonsense input scrolls nothing', () => {
  const c = new LineCarry();
  assert.equal(c.add(50, 0), 0);
  assert.equal(c.add(50, NaN), 0);
  assert.equal(c.add(NaN, 20), 0);
  assert.equal(c.add(Infinity, 20), 0);
  assert.equal(c.add(20, 20), 1, 'and it still works afterwards');
});

test('speed comes from the last moments of the drag', () => {
  let s = addSample([], { y: 0, t: 0 });
  s = addSample(s, { y: 30, t: 50 });
  s = addSample(s, { y: 90, t: 100 });
  assert.equal(velocity(s, 100), 0.9, 'px per ms');
  assert.equal(velocity(s, 400), 0, 'a finger that rested before lifting does not fling');
  assert.equal(velocity([], 0), 0);
  assert.equal(velocity([{ y: 5, t: 5 }], 5), 0, 'one sample has no speed');
  assert.equal(velocity(addSample([], { y: 0, t: 10 }), 10), 0);
  const up = addSample(addSample([], { y: 100, t: 0 }), { y: 40, t: 60 });
  assert.equal(velocity(up, 60), -1);
});

test('old samples fall out of the window', () => {
  let s = addSample([], { y: 0, t: 0 });
  s = addSample(s, { y: 500, t: 1000 });
  assert.equal(s.length, 1);
});

test('a flick is capped, a slow lift is not a fling', () => {
  const s = addSample(addSample([], { y: 0, t: 0 }), { y: 1000, t: 10 });
  assert.equal(velocity(s, 10), 6);
  assert.equal(flings(0.2), false);
  assert.equal(flings(-0.5), true);
  assert.equal(keepGoing(0.02), false);
  assert.equal(keepGoing(-0.1), true);
});

test('a fling slows down and stops', () => {
  let v = 2;
  let frames = 0;
  while (keepGoing(v) && frames < 1000) {
    v = slow(v, 16);
    frames++;
  }
  assert.ok(frames > 20 && frames < 150, `${frames} frames`);
  assert.ok(slow(1, 32) < slow(1, 16), 'a longer frame loses more');
  assert.equal(slow(-1, 0), -1);
});

test('lines per request are capped', () => {
  assert.equal(clampLines(5.9), 5);
  assert.equal(clampLines(-5.9), -5);
  assert.equal(clampLines(9999), 120);
  assert.equal(clampLines(-9999), -120);
});

test('typing leaves the history before it reaches the agent', () => {
  const enc = (s: string) => new TextEncoder().encode(s);
  assert.deepEqual(inputWhileScrolled(enc('h')), { leave: true, forward: true });
  assert.deepEqual(inputWhileScrolled(enc('\r')), { leave: true, forward: true });
  assert.deepEqual(inputWhileScrolled(enc('\x7f')), { leave: true, forward: true });
  assert.deepEqual(inputWhileScrolled(enc('\x03')), { leave: true, forward: true }, 'Ctrl-C is typing');
  assert.deepEqual(inputWhileScrolled(enc('é')), { leave: true, forward: true });
});

test('a lone Escape only leaves the history; it must not interrupt the agent', () => {
  assert.deepEqual(inputWhileScrolled(new Uint8Array([0x1b])), { leave: true, forward: false });
});

test('escape sequences are not typing', () => {
  const enc = (s: string) => new TextEncoder().encode(s);
  for (const seq of ['\x1b[A', '\x1b[I', '\x1b[O', '\x1b[?62;4c', '\x1b[<0;5;5M', '\x1bOP']) {
    assert.deepEqual(inputWhileScrolled(enc(seq)), { leave: false, forward: true }, JSON.stringify(seq));
  }
  assert.deepEqual(inputWhileScrolled(new Uint8Array()), { leave: false, forward: true });
});

test('the button says how far back the screen is', () => {
  assert.equal(liveLabel(0), 'Back to live');
  assert.equal(liveLabel(1), 'Back to live · 1 line up');
  assert.equal(liveLabel(120), 'Back to live · 120 lines up');
});
