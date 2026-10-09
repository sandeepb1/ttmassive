import { test } from 'node:test';
import assert from 'node:assert/strict';
import { applyMods, encodeKey } from './keys.ts';

const none = { ctrl: false, alt: false };
const ctrl = { ctrl: true, alt: false };
const alt = { ctrl: false, alt: true };

test('plain arrows use CSI, or SS3 in application cursor mode', () => {
  assert.equal(encodeKey('Up', none), '\x1b[A');
  assert.equal(encodeKey('Left', none), '\x1b[D');
  assert.equal(encodeKey('Up', none, true), '\x1bOA');
});

test('modified arrows use xterm modifier parameters regardless of DECCKM', () => {
  assert.equal(encodeKey('Right', ctrl, true), '\x1b[1;5C');
  assert.equal(encodeKey('Left', alt), '\x1b[1;3D');
  assert.equal(encodeKey('Down', { ctrl: true, alt: true }), '\x1b[1;7B');
});

test('Esc, Tab, Shift-Tab, paging', () => {
  assert.equal(encodeKey('Esc', none), '\x1b');
  assert.equal(encodeKey('Tab', none), '\t');
  assert.equal(encodeKey('ShiftTab', none), '\x1b[Z');
  assert.equal(encodeKey('PgUp', none), '\x1b[5~');
  assert.equal(encodeKey('PgDn', ctrl), '\x1b[6;5~');
});

test('ctrl + letter produces control bytes', () => {
  assert.equal(applyMods('c', ctrl), '\x03');
  assert.equal(applyMods('C', ctrl), '\x03');
  assert.equal(applyMods('d', ctrl), '\x04');
  assert.equal(applyMods('[', ctrl), '\x1b');
  assert.equal(applyMods(' ', ctrl), '\x00');
});

test('alt prefixes ESC; combined with ctrl', () => {
  assert.equal(applyMods('b', alt), '\x1bb');
  assert.equal(applyMods('b', { ctrl: true, alt: true }), '\x1b\x02');
});

test('multi-character input and unmapped keys pass through', () => {
  assert.equal(applyMods('hello', ctrl), 'hello');
  assert.equal(applyMods('1', ctrl), '1');
  assert.equal(applyMods('x', none), 'x');
});
