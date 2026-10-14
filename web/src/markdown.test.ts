import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseInline, parseMarkdown, previewInlines, type Block, type Inline } from './markdown.ts';

/** Inline tree as readable text: `**bold**` → `[b:bold]`, links → `[a href:text]`. */
function show(c: Inline[]): string {
  return c
    .map((n) => {
      switch (n.t) {
        case 'text':
          return n.v;
        case 'code':
          return `[code:${n.v}]`;
        case 'strong':
          return `[b:${show(n.c)}]`;
        case 'em':
          return `[i:${show(n.c)}]`;
        case 'del':
          return `[s:${show(n.c)}]`;
        case 'link':
          return `[a ${n.href}:${show(n.c)}]`;
        case 'br':
          return '\\n';
      }
    })
    .join('');
}

const first = (src: string): Block => parseMarkdown(src)[0];

test('paragraphs split on blank lines and keep single newlines as line breaks', () => {
  const blocks = parseMarkdown('one\ntwo\n\nthree');
  assert.equal(blocks.length, 2);
  assert.equal(blocks[0].t === 'p' && show(blocks[0].c), 'one\\ntwo');
  assert.equal(blocks[1].t === 'p' && show(blocks[1].c), 'three');
});

test('headings and rules', () => {
  const h = first('## Title ##');
  assert.deepEqual(h.t === 'h' && [h.level, show(h.c)], [2, 'Title']);
  assert.equal(first('---').t, 'hr');
  assert.equal(first('#nospace').t, 'p'); // not a heading
});

test('fenced code keeps its text and language, and survives a missing closing fence', () => {
  const c = first('```rust\nfn main() {}\n\n  indented\n```');
  assert.deepEqual(c, { t: 'code', lang: 'rust', v: 'fn main() {}\n\n  indented' });
  const open = parseMarkdown('before\n```\nnever closed\nstill code');
  assert.deepEqual(open[1], { t: 'code', lang: '', v: 'never closed\nstill code' });
  // Markdown inside a fence is not interpreted.
  assert.deepEqual(first('```\n**x** <b>\n```'), { t: 'code', lang: '', v: '**x** <b>' });
});

test('inline code, emphasis, strikethrough', () => {
  assert.equal(show(parseInline('a `b*c*` **bold** *it* ~~gone~~')), 'a [code:b*c*] [b:bold] [i:it] [s:gone]');
  assert.equal(show(parseInline('nested **bold and *italic***')), 'nested [b:bold and [i:italic]]');
  assert.equal(show(parseInline('``a ` b``')), '[code:a ` b]');
});

test('emphasis does not trigger on arithmetic, snake_case or lone markers', () => {
  assert.equal(show(parseInline('2 * 3 * 4')), '2 * 3 * 4');
  assert.equal(show(parseInline('use snake_case_names here')), 'use snake_case_names here');
  assert.equal(show(parseInline('a * b')), 'a * b');
  assert.equal(show(parseInline('unclosed **bold')), 'unclosed **bold');
});

test('backslash escapes', () => {
  assert.equal(show(parseInline('\\*not italic\\* and \\`not code\\`')), '*not italic* and `not code`');
});

test('links: http(s) and mailto only, bare URLs are linked without trailing punctuation', () => {
  assert.equal(show(parseInline('[docs](https://a.dev/x)')), '[a https://a.dev/x:docs]');
  assert.equal(show(parseInline('see https://a.dev/x, ok')), 'see [a https://a.dev/x:https://a.dev/x], ok');
  assert.equal(show(parseInline('(https://a.dev/x)')), '([a https://a.dev/x:https://a.dev/x])');
  assert.equal(show(parseInline('[mail](mailto:a@b.dev)')), '[a mailto:a@b.dev:mail]');
});

test('unsafe links become plain text', () => {
  assert.equal(show(parseInline('[click](javascript:alert(1))')), 'click');
  assert.equal(show(parseInline('[x](data:text/html;base64,AAAA)')), 'x');
  assert.equal(show(parseInline('[x](//evil.test)')), 'x');
});

test('raw HTML is just text, never markup', () => {
  const p = first('<script>alert(1)</script> <img src=x onerror=alert(1)>');
  assert.equal(p.t === 'p' && show(p.c), '<script>alert(1)</script> <img src=x onerror=alert(1)>');
  assert.equal(p.t === 'p' && p.c.every((n) => n.t === 'text'), true);
});

test('bullet and numbered lists, nested lists, and item continuation', () => {
  const l = first('- a\n- b\n  - b1\n  - b2\n- c');
  assert.equal(l.t, 'list');
  if (l.t !== 'list') return;
  assert.equal(l.ordered, false);
  assert.equal(l.items.length, 3);
  const nested = l.items[1].find((b) => b.t === 'list');
  assert.equal(nested?.t === 'list' && nested.items.length, 2);

  const n = first('3. three\n4. four');
  assert.deepEqual(n.t === 'list' && [n.ordered, n.start, n.items.length], [true, 3, 2]);

  const spaced = first('- a\n\n- b');
  assert.equal(spaced.t === 'list' && spaced.items.length, 2);
});

test('a list item can hold a code block', () => {
  const l = first('1. run:\n   ```sh\n   make test\n   ```\n2. done');
  assert.equal(l.t === 'list' && l.items.length, 2);
  const code = l.t === 'list' ? l.items[0].find((b) => b.t === 'code') : undefined;
  assert.deepEqual(code, { t: 'code', lang: 'sh', v: 'make test' });
});

test('block quotes hold blocks', () => {
  const q = first('> quoted\n> **more**\n\nafter');
  assert.equal(q.t, 'quote');
  assert.equal(q.t === 'quote' && q.c[0].t, 'p');
});

test('tables', () => {
  const t = first('| a | b |\n|---|:-:|\n| 1 | `x` |\n| 2 | y \\| z |');
  assert.equal(t.t, 'table');
  if (t.t !== 'table') return;
  assert.deepEqual(t.head.map(show), ['a', 'b']);
  assert.equal(t.rows.length, 2);
  assert.equal(show(t.rows[0][1]), '[code:x]');
  assert.equal(show(t.rows[1][1]), 'y | z');
  // A lone pipe line is not a table.
  assert.equal(first('a | b').t, 'p');
});

test('empty and whitespace input', () => {
  assert.deepEqual(parseMarkdown(''), []);
  assert.deepEqual(parseMarkdown('\n  \n'), []);
});

test('windows line endings', () => {
  assert.equal(parseMarkdown('a\r\n\r\nb').length, 2);
});

test('a preview keeps bold and code, bullets list items and puts lines apart', () => {
  const r = previewInlines('Tick **Media playback** only.\n- **Media playback:** the app plays `mp3`');
  assert.deepEqual(r[0], { t: 'text', v: 'Tick ' });
  assert.deepEqual(r[1], { t: 'strong', c: [{ t: 'text', v: 'Media playback' }] });
  assert.ok(r.some((n) => n.t === 'br'), 'the lines are separated');
  assert.ok(r.some((n) => n.t === 'text' && n.v === '• '), 'the list item has a bullet');
  assert.ok(r.some((n) => n.t === 'code' && n.v === 'mp3'));
});

test('a preview has no links, headings are bold and code blocks are one line', () => {
  const r = previewInlines('# Done\nSee [the docs](https://example.com/x) now\n```\nlet a = 1;\nlet b = 2;\n```');
  assert.ok(!JSON.stringify(r).includes('"link"'), 'links are flattened');
  assert.deepEqual(r[0], { t: 'strong', c: [{ t: 'text', v: 'Done' }] });
  assert.ok(r.some((n) => n.t === 'code' && n.v === 'let a = 1; let b = 2;'));
});

test('a preview of plain text is that text', () => {
  assert.deepEqual(previewInlines('all quiet'), [{ t: 'text', v: 'all quiet' }]);
  assert.deepEqual(previewInlines(''), []);
});
