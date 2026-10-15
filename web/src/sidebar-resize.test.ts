import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clampWidth, effectiveWidth, parseWidths, DEFAULT_W, MIN_W, MAX_W } from './sidebar-resize.ts';

test('a width stays inside the side’s own range', () => {
  assert.equal(clampWidth('left', 10, 0, 1920), MIN_W.left);
  assert.equal(clampWidth('left', 5000, 0, 1920), MAX_W.left);
  assert.equal(clampWidth('right', 350, 0, 1920), 350);
});

test('the conversation keeps its room beside the other sidebar', () => {
  // 1100 - 320 - 400 = 380 left for the list
  assert.equal(clampWidth('left', 560, 320, 1100), 380);
  // never below the minimum, even in a window too small for both
  assert.equal(clampWidth('left', 400, 320, 800), MIN_W.left);
});

test('saved widths are read back, bad ones fall to the default', () => {
  assert.deepEqual(parseWidths('{"left":400,"right":280}'), { left: 400, right: 280 });
  assert.deepEqual(parseWidths('{"left":"wide","right":99999}'), { left: DEFAULT_W.left, right: MAX_W.right });
  assert.deepEqual(parseWidths('nonsense'), DEFAULT_W);
  assert.deepEqual(parseWidths(null), DEFAULT_W);
});

test('an overlay sidebar is never squeezed by the window it slides over', () => {
  // a phone: 411px wide, nothing beside the conversation
  assert.equal(effectiveWidth('left', 300, false, 0, 411), 300);
  assert.equal(effectiveWidth('right', 320, false, 0, 411), 320);
  // the same window as a pinned column has to leave the conversation its room
  assert.equal(effectiveWidth('left', 300, true, 0, 411), MIN_W.left);
});
