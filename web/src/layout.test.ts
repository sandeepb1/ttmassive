import { test } from 'node:test';
import assert from 'node:assert/strict';
import { closeOverlays, INITIAL, placement, setOpen, toggled, visible } from './layout.ts';

test('phone: both sidebars are overlays', () => {
  assert.equal(placement('left', 390, 844), 'overlay');
  assert.equal(placement('right', 390, 844), 'overlay');
});

test('tablet portrait pins the list only; landscape and desktop pin both', () => {
  assert.equal(placement('left', 800, 1280), 'pinned');
  assert.equal(placement('right', 800, 1280), 'overlay');
  assert.equal(placement('left', 1280, 800), 'pinned');
  assert.equal(placement('right', 1280, 800), 'pinned');
});

test('a short landscape phone does not pin a column', () => {
  assert.equal(placement('left', 844, 390), 'overlay');
  assert.equal(placement('right', 1180, 390), 'overlay');
});

test('overlays start closed and pinned sidebars start visible', () => {
  assert.equal(visible(INITIAL, 'left', 390, 844), false);
  assert.equal(visible(INITIAL, 'left', 1280, 800), true);
});

test('toggling a pinned sidebar collapses it and brings it back', () => {
  const a = toggled(INITIAL, 'left', 1280, 800);
  assert.equal(a.left.collapsed, true);
  assert.equal(visible(a, 'left', 1280, 800), false);
  assert.equal(visible(toggled(a, 'left', 1280, 800), 'left', 1280, 800), true);
});

test('opening one overlay closes the other', () => {
  const a = toggled(INITIAL, 'left', 390, 844);
  assert.equal(a.left.open, true);
  const b = toggled(a, 'right', 390, 844);
  assert.equal(b.right.open, true);
  assert.equal(b.left.open, false);
});

test('setOpen does nothing when it is already so', () => {
  assert.equal(setOpen(INITIAL, 'left', false, 390, 844), INITIAL);
  assert.equal(setOpen(INITIAL, 'left', true, 390, 844).left.open, true);
});

test('closeOverlays keeps collapsed choices', () => {
  const s = closeOverlays({ left: { collapsed: true, open: true }, right: { collapsed: false, open: true } });
  assert.deepEqual(s, { left: { collapsed: true, open: false }, right: { collapsed: false, open: false } });
});

test('resizing from a phone to a desktop shows a pinned sidebar even if an overlay was left open', () => {
  const open = toggled(INITIAL, 'left', 390, 844);
  assert.equal(visible(open, 'left', 1280, 800), true);
  assert.equal(visible(toggled(INITIAL, 'left', 1280, 800), 'left', 390, 844), false);
});
