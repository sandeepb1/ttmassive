import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clampScale, SCALE_DEFAULT, SCALE_MAX, SCALE_MIN } from './scale.ts';

test('scale snaps to steps and stays in range', () => {
  assert.equal(clampScale(100), 100);
  assert.equal(clampScale(102), 100);
  assert.equal(clampScale(103), 105);
  assert.equal(clampScale(10), SCALE_MIN);
  assert.equal(clampScale(500), SCALE_MAX);
  assert.equal(clampScale(NaN), SCALE_DEFAULT);
});
