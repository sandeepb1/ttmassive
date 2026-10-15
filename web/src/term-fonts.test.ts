import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cleanFamily, fontStack, hasLigatures, termFont } from './term-fonts.ts';

test('a bundled font leads its stack, with a monospace behind it', () => {
  assert.match(fontStack('fira', ''), /^"Fira Code Variable", .*monospace$/);
  assert.match(fontStack('system', ''), /^ui-monospace/);
});

test('an unknown id falls back to JetBrains Mono', () => {
  assert.equal(termFont('nonsense').id, 'jetbrains');
});

test('a typed family cannot break out of the CSS value', () => {
  assert.equal(cleanFamily('  Berkeley   Mono "; } body { x'), 'Berkeley Mono body x');
  assert.equal(cleanFamily('a'.repeat(200)).length, 64);
  assert.ok(fontStack('custom', 'MesloLGS NF').startsWith('"MesloLGS NF", '));
  assert.ok(!fontStack('custom', '').startsWith('"'));
});

test('only fonts that have ligatures offer them', () => {
  assert.equal(hasLigatures('fira', ''), true);
  assert.equal(hasLigatures('source', ''), false);
  assert.equal(hasLigatures('custom', ''), false);
  assert.equal(hasLigatures('custom', 'Iosevka'), true);
});
