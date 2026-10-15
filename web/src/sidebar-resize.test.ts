import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clampWidth, parseWidths, DEFAULT_W, MIN_W, MAX_W } from './sidebar-resize.ts';

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
