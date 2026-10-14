// Keeps the UI uniform: the rules in web/UI.md, checked against the source. When one of these fails,
// the message says what to do instead. See UI.md before adding to an allowance.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';

const SRC = join(import.meta.dirname, '..');

function walk(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name);
    return e.isDirectory() ? walk(p) : [p];
  });
}

const files = walk(SRC).map((p) => ({ path: relative(SRC, p), text: readFileSync(p, 'utf8') }));
const ts = files.filter((f) => f.path.endsWith('.ts') && !f.path.endsWith('.test.ts'));
const css = files.filter((f) => f.path.endsWith('.css'));

/** The line a match is on, for the failure message. */
const lineOf = (text: string, index: number) => text.slice(0, index).split('\n').length;

// ---------------------------------------------------------------------------------------------

/** Files that may hold a raw colour, and why. Everything else uses a token (var(--...) or a Tailwind colour). */
const HEX_OK = new Set([
  'theme.ts', // the palettes: the one place colours are defined
  'image.ts', // paints a canvas white before compressing a picture
  'ui/app.css', // the first-paint fallback palette, before theme.ts has run
  'ui/viewer.css', // the image viewer is a fixed dark overlay in every theme
]);

test('no raw hex colours outside the palette and its fallbacks', () => {
  const bad: string[] = [];
  for (const f of [...ts, ...css]) {
    if (HEX_OK.has(f.path)) continue;
    for (const m of f.text.matchAll(/#[0-9a-fA-F]{3,8}\b/g)) bad.push(`${f.path}:${lineOf(f.text, m.index)} ${m[0]}`);
  }
  assert.deepEqual(bad, [], 'Use a theme token (bg-card, text-muted-foreground, var(--fg)...), not a colour literal.');
});

// ---------------------------------------------------------------------------------------------

test('no arbitrary radius, spacing or gap values: use the scale', () => {
  const arbitrary = /(?<![\w-])-?(?:rounded(?:-[a-z]{1,2})?|[pm][xytblrse]?|gap(?:-[xy])?|space-[xy])-\[-?\d*\.?\d+(?:px|rem|em)\]/g;
  const bad: string[] = [];
  for (const f of [...ts, ...css]) {
    for (const m of f.text.matchAll(arbitrary)) bad.push(`${f.path}:${lineOf(f.text, m.index)} ${m[0]}`);
  }
  assert.deepEqual(bad, [], 'Use a step from the scale (rounded-lg, p-3, gap-2) or add a token in ui/tailwind.css.');
});

// ---------------------------------------------------------------------------------------------

/**
 * Raw `el('button', ...)` calls per file. These are composite rows and tabs with their own structure
 * and a CSS hook class (a session row, a tab with a badge). Anything that is just a button, chip,
 * row, input or toggle goes through src/ui. The numbers can only go down: lower one when you
 * convert a button, and do not raise one without a reason a reviewer would accept.
 */
const RAW_BUTTONS: Record<string, number> = {
  'chat-render.ts': 2,
  'chat.ts': 3,
  'image-viewer.ts': 2,
  'layout-sheet.ts': 1,
  'main.ts': 1,
  'new-session-sheet.ts': 4,
  'pane.ts': 1,
  'rightbar.ts': 5,
  'sessions.ts': 5,
  'term-search.ts': 2,
  'term.ts': 2,
  'theme-picker.ts': 1,
};

test('plain buttons come from src/ui; raw button counts only go down', () => {
  const found: Record<string, number> = {};
  for (const f of ts) {
    if (f.path.startsWith('ui/')) continue;
    const n = [...f.text.matchAll(/el\(\s*'button'/g)].length;
    if (n > 0) found[f.path] = n;
  }
  const over = Object.entries(found).filter(([p, n]) => n > (RAW_BUTTONS[p] ?? 0));
  assert.deepEqual(over, [], "Use button(), chip(), row() or toggleSwitch() from './ui/...' instead of el('button').");
  const stale = Object.entries(RAW_BUTTONS).filter(([p, n]) => (found[p] ?? 0) < n);
  assert.deepEqual(stale, [], 'A file now has fewer raw buttons than its allowance: lower it in RAW_BUTTONS.');
});

// ---------------------------------------------------------------------------------------------

/** Hook classes carried by kit buttons that CSS hides. Add a class here when you hide another. */
const KIT_HOOKS = ['p-menu', 'p-panel', 'p-split', 'p-close', 'p-layout', 'p-cmd', 'p-new', 'ts-open', 't-toast', 'sb-collapse', 'rail-expand', 'rail-settings', 'srow-more', 'sg-act', 'rb-refresh'];

test('CSS that hides a kit element says !important (a display utility in a later layer would win)', () => {
  const hooks = new RegExp(`\\.(?:${KIT_HOOKS.join('|')})(?![\\w-])`);
  const bad: string[] = [];
  for (const f of css) {
    for (const m of f.text.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const [, selector, body] = m;
      if (!hooks.test(selector!) || selector!.includes('::')) continue;
      if (/display:\s*none(?!\s*!important)/.test(body!)) bad.push(`${f.path}:${lineOf(f.text, m.index)} ${selector!.trim()}`);
    }
  }
  assert.deepEqual(bad, [], 'Write `display: none !important` for rules that hide kit buttons.');
});

// ---------------------------------------------------------------------------------------------

test('every stylesheet in src/ui is imported by tailwind.css', () => {
  const entry = files.find((f) => f.path === 'ui/tailwind.css')!.text;
  const imported = new Set([...entry.matchAll(/@import '\.\/([\w-]+\.css)'/g)].map((m) => m[1]));
  const orphans = css.map((f) => f.path.replace('ui/', '')).filter((n) => n !== 'tailwind.css' && !imported.has(n));
  assert.deepEqual(orphans, [], 'Add an @import for it in ui/tailwind.css, or delete it.');
});
