import assert from 'node:assert/strict';
import { test } from 'node:test';
import { defaultLayouts, paneLimit, parseLayouts } from './workspace.ts';

test('anything that is not a document gives the defaults', () => {
  for (const raw of [null, undefined, 3, 'x', [], {}]) assert.deepEqual(parseLayouts(raw), defaultLayouts());
});

test('settings are read field by field', () => {
  const l = parseLayouts({ settings: { multi: false, maxPanes: 99, desktop: 'grid2x2', tablet: 'bogus' } });
  assert.equal(l.settings.multi, false);
  assert.equal(l.settings.maxPanes, 6); // clamped
  assert.equal(l.settings.desktop, 'grid2x2');
  assert.equal(l.settings.tablet, 'cols2'); // unknown preset: default
  assert.equal(parseLayouts({ settings: { maxPanes: 0 } }).settings.maxPanes, 1);
  assert.equal(parseLayouts({ settings: { maxPanes: 2.5 } }).settings.maxPanes, 4);
});

test('workspaces keep what is usable', () => {
  const l = parseLayouts({
    workspaces: [
      { name: 'Dev', preset: 'cols2', cols: [3, 1], rows: [1], tiles: ['claude', '', 'bad id!', 7] },
      { name: '', preset: 'cols2', cols: [1, 1], rows: [1], tiles: [] },
      { name: 'No preset', preset: 'nope', cols: [1], rows: [1], tiles: [] },
      'junk',
    ],
  });
  assert.equal(l.workspaces.length, 1);
  const w = l.workspaces[0]!;
  assert.deepEqual(w.tiles, ['claude', '']); // bad entries dropped
  assert.deepEqual(w.cols, [0.75, 0.25]);
});

test('sizes that do not fit the preset are reset', () => {
  const w = parseLayouts({ workspaces: [{ name: 'x', preset: 'grid2x2', cols: [1], rows: 'no', tiles: [] }] }).workspaces[0]!;
  assert.deepEqual(w.cols, [0.5, 0.5]);
  assert.deepEqual(w.rows, [0.5, 0.5]);
});

test('the pane limit is one when multi-pane is off', () => {
  assert.equal(paneLimit({ multi: false, maxPanes: 5, desktop: 'cols2', tablet: 'cols2' }), 1);
  assert.equal(paneLimit({ multi: true, maxPanes: 5, desktop: 'cols2', tablet: 'cols2' }), 5);
});

test('pinned projects keep what is usable', () => {
  const l = parseLayouts({ pinnedProjects: ['/w/app', '/w/app', '', 3, 'a\nb', '~/code'] });
  assert.deepEqual(l.pinnedProjects, ['/w/app', '~/code']);
  assert.deepEqual(parseLayouts({}).pinnedProjects, []);
});
