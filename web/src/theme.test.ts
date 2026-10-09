import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  contrast,
  DEFAULT_DARK,
  DEFAULT_LIGHT,
  DEFAULT_PREF,
  mix,
  mutedPct,
  onAccent,
  resolveTheme,
  THEMES,
  themeById,
  xtermTheme,
} from './theme.ts';

const HEX = /^#[0-9a-f]{6}$/i;

test('theme ids are unique and every colour is a 6-digit hex', () => {
  assert.equal(new Set(THEMES.map((t) => t.id)).size, THEMES.length);
  for (const t of THEMES) {
    for (const c of [t.bg, t.fg, t.accent, t.ok, t.warn, t.danger, t.claude, t.codex, ...t.ansi]) {
      assert.match(c, HEX, `${t.id}: ${c}`);
    }
    assert.equal(t.ansi.length, 16, t.id);
  }
});

test('the defaults exist, with the right scheme', () => {
  assert.equal(themeById(DEFAULT_DARK)?.scheme, 'dark');
  assert.equal(themeById(DEFAULT_LIGHT)?.scheme, 'light');
  assert.ok(THEMES.some((t) => t.scheme === 'light') && THEMES.some((t) => t.scheme === 'dark'));
});

test('body text, muted text and accents stay readable on every background', () => {
  for (const t of THEMES) {
    assert.ok(contrast(t.fg, t.bg) >= 7, `${t.id}: text ${contrast(t.fg, t.bg).toFixed(2)}`);
    // style.css derives --fg-muted as mutedPct(t) of fg over bg.
    const muted = mix(t.bg, t.fg, mutedPct(t));
    assert.ok(contrast(muted, t.bg) >= 4.5, `${t.id}: muted ${contrast(muted, t.bg).toFixed(2)}`);
    for (const [name, c] of Object.entries({ accent: t.accent, ok: t.ok, warn: t.warn, danger: t.danger, claude: t.claude, codex: t.codex })) {
      assert.ok(contrast(c, t.bg) >= 3, `${t.id}: ${name} ${contrast(c, t.bg).toFixed(2)}`);
    }
  }
});

test('text on filled accent buttons is readable for every agent accent', () => {
  for (const t of THEMES) {
    const on = onAccent(t);
    for (const a of [t.accent, t.claude, t.codex]) {
      assert.ok(contrast(on, a) >= 4.5, `${t.id}: ${on} on ${a} ${contrast(on, a).toFixed(2)}`);
    }
  }
});

test('auto picks the light or dark theme; unknown ids fall back', () => {
  const pref = { ...DEFAULT_PREF, auto: true, light: 'catppuccin-latte', dark: 'nord' };
  assert.equal(resolveTheme(pref, true).id, 'nord');
  assert.equal(resolveTheme(pref, false).id, 'catppuccin-latte');
  assert.equal(resolveTheme({ ...DEFAULT_PREF, theme: 'gone' }, true).id, DEFAULT_DARK);
  assert.equal(resolveTheme({ ...DEFAULT_PREF, auto: true, light: 'gone' }, false).id, DEFAULT_LIGHT);
});

test('mix matches color-mix(in srgb)', () => {
  assert.equal(mix('#000000', '#ffffff', 50), '#808080');
  assert.equal(mix('#102030', '#102030', 70), '#102030');
});

test('the xterm theme carries the full palette', () => {
  const x = xtermTheme(themeById('nord')!);
  assert.equal(x.background, '#2e3440');
  assert.equal(x.red, '#bf616a');
  assert.equal(x.brightWhite, '#eceff4');
});
