import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  corners,
  dividers,
  dragSizes,
  effectivePreset,
  fits,
  neighbor,
  normalize,
  preset,
  presetClass,
  presetForCount,
  sanitizeSizes,
  span,
  tileCount,
  tileNames,
  visiblePanes,
} from './tiling.ts';

test('tile names follow reading order and merge repeated areas', () => {
  assert.deepEqual(tileNames(preset('main-side')), ['a', 'b', 'c']);
  assert.equal(tileCount(preset('grid3x2')), 6);
  assert.equal(tileCount(preset('single')), 1);
});

test('unknown preset falls back to a single pane', () => {
  assert.equal(preset('nope').id, 'single');
});

test('fit depends on the area, not the device', () => {
  assert.ok(fits(preset('cols2'), 800, 600));
  assert.ok(!fits(preset('cols2'), 700, 600));
  assert.ok(fits(preset('rows2'), 600, 600));
  assert.ok(!fits(preset('rows2'), 500, 600)); // a phone: tabs, not a squeezed stack
  assert.ok(!fits(preset('grid2x2'), 1000, 500));
});

test('a preset that does not fit steps down to the biggest that does', () => {
  assert.equal(effectivePreset('grid2x2', 1400, 900, 6).id, 'grid2x2');
  assert.equal(effectivePreset('grid3x2', 900, 600, 6).id, 'grid2x2'); // 4 tiles beats 3 or 2
  assert.equal(effectivePreset('main-side', 700, 600, 6).id, 'rows2'); // too narrow for columns
  assert.equal(effectivePreset('grid2x2', 600, 700, 6).id, 'rows2'); // portrait tablet: stacked
  assert.equal(effectivePreset('grid2x2', 420, 800, 6).id, 'single'); // phone: one pane, the rest in tabs
  assert.equal(effectivePreset('grid2x2', 360, 400, 6).id, 'single');
});

test('the pane cap limits presets', () => {
  assert.equal(effectivePreset('grid2x2', 1600, 1000, 2).id, 'cols2');
  assert.equal(effectivePreset('grid2x2', 1600, 1000, 1).id, 'single');
});

test('adding a pane bumps to the smallest preset with room', () => {
  assert.equal(presetForCount('cols2', 2, 1600, 1000, 6), 'cols2');
  assert.equal(presetForCount('cols2', 3, 1600, 1000, 6), 'cols3');
  assert.equal(presetForCount('cols2', 4, 1600, 1000, 6), 'grid2x2');
  // Nothing bigger fits: stay put and let the extra pane go behind a tab.
  assert.equal(presetForCount('single', 2, 360, 400, 6), 'single');
});

test('visible panes always include the focused one', () => {
  assert.deepEqual(visiblePanes(3, 0, 2), [0, 1]);
  assert.deepEqual(visiblePanes(3, 2, 2), [0, 2]);
  assert.deepEqual(visiblePanes(1, 0, 4), [0]);
  assert.deepEqual(visiblePanes(4, 3, 1), [3]);
  assert.deepEqual(visiblePanes(0, 0, 2), []);
});

test('corner tiles hold the sidebar buttons', () => {
  assert.deepEqual(corners(preset('cols3')), { first: 'a', last: 'c' });
  assert.deepEqual(corners(preset('rows2')), { first: 'a', last: 'a' });
  assert.deepEqual(corners(preset('main-side')), { first: 'a', last: 'b' });
});

test('sizes normalize, with a safe fallback', () => {
  assert.deepEqual(normalize([1, 1]), [0.5, 0.5]);
  assert.deepEqual(normalize([2, 1, 1]), [0.5, 0.25, 0.25]);
  assert.deepEqual(normalize([1, -1]), [0.5, 0.5]);
  assert.deepEqual(normalize([NaN, 1]), [0.5, 0.5]);
});

test('saved sizes that do not match the preset are ignored', () => {
  const p = preset('cols3');
  assert.deepEqual(sanitizeSizes(p, [1, 2], [1]).cols, [1 / 3, 1 / 3, 1 / 3]);
  assert.deepEqual(sanitizeSizes(p, [2, 1, 1], [1]).cols, [0.5, 0.25, 0.25]);
  assert.deepEqual(sanitizeSizes(p, 'x', null).rows, [1]);
});

test('dragging a divider moves space between neighbours and stops at the minimum', () => {
  const near = (a: number[], b: number[]) => a.forEach((x, i) => assert.ok(Math.abs(x - b[i]!) < 1e-9, `${a} vs ${b}`));
  near(dragSizes([0.5, 0.5], 0, 0.1), [0.6, 0.4]);
  near(dragSizes([0.5, 0.5], 0, -0.1), [0.4, 0.6]);
  near(dragSizes([0.5, 0.5], 0, 0.9), [0.88, 0.12]);
  near(dragSizes([0.5, 0.5], 0, -0.9), [0.12, 0.88]);
  near(dragSizes([0.2, 0.3, 0.5], 1, 0.1), [0.2, 0.4, 0.4]);
  assert.deepEqual(dragSizes([1], 0, 0.1), [1]);
});

test('a big tile has no divider through it', () => {
  const d = dividers(preset('main-side'));
  assert.deepEqual(d, [
    { axis: 'col', after: 0, from: 0, to: 1 }, // between the big tile and the stack
    { axis: 'row', after: 0, from: 1, to: 1 }, // only across the right-hand stack
  ]);
});

test('grids get full-length dividers', () => {
  assert.deepEqual(dividers(preset('grid2x2')), [
    { axis: 'col', after: 0, from: 0, to: 1 },
    { axis: 'row', after: 0, from: 0, to: 1 },
  ]);
  assert.deepEqual(dividers(preset('single')), []);
  assert.equal(dividers(preset('grid3x2')).length, 3);
});

test('span turns tracks into a fraction range', () => {
  assert.deepEqual(span([0.5, 0.25, 0.25], 1, 2), [0.5, 1]);
  assert.deepEqual(span([0.5, 0.5], 0, 0), [0, 0.5]);
});

test('moving between tiles', () => {
  const g = preset('grid2x2');
  assert.equal(neighbor(g, 0, 'right'), 1);
  assert.equal(neighbor(g, 0, 'down'), 2);
  assert.equal(neighbor(g, 3, 'left'), 2);
  assert.equal(neighbor(g, 3, 'up'), 1);
  assert.equal(neighbor(g, 0, 'left'), null);
  assert.equal(neighbor(g, 0, 'up'), null);
  assert.equal(neighbor(g, 3, 'down'), null);
  assert.equal(neighbor(g, 9, 'left'), null);
});

test('a big tile steps past itself', () => {
  const m = preset('main-side'); // a b / a c
  assert.equal(neighbor(m, 0, 'right'), 1);
  assert.equal(neighbor(m, 0, 'down'), null);
  assert.equal(neighbor(m, 1, 'down'), 2);
  assert.equal(neighbor(m, 2, 'left'), 0);
  assert.equal(neighbor(m, 2, 'up'), 1);
});

test('the starting preset follows the screen size', () => {
  assert.equal(presetClass(1920), 'desktop');
  assert.equal(presetClass(1280), 'tablet');
});
