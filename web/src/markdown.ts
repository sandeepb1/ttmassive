// A small Markdown parser for agent replies. It produces a tree, never HTML: the chat view
// builds DOM nodes from it with `textContent`, so nothing the agent prints can inject markup.
// Raw HTML such as `<script>` stays literal text. Links are kept only for http(s) and mailto.
//
// Supported: paragraphs, headings, fenced code, bullet and numbered lists (nested), quotes,
// rules, tables, and inline code, bold, italic, strikethrough, links and bare URLs.

export type Inline =
  | { t: 'text'; v: string }
  | { t: 'code'; v: string }
  | { t: 'strong'; c: Inline[] }
  | { t: 'em'; c: Inline[] }
  | { t: 'del'; c: Inline[] }
  | { t: 'link'; href: string; c: Inline[] }
  | { t: 'br' };

export type Block =
  | { t: 'p'; c: Inline[] }
  | { t: 'h'; level: number; c: Inline[] }
  | { t: 'code'; lang: string; v: string }
  | { t: 'list'; ordered: boolean; start: number; items: Block[][] }
  | { t: 'quote'; c: Block[] }
  | { t: 'hr' }
  | { t: 'table'; head: Inline[][]; rows: Inline[][][] };

const SAFE_HREF = /^(https?:\/\/|mailto:)/i;
const BARE_URL = /^https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"\]]/i;
const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^\s`]*)[^`]*$/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const BULLET = /^(\s*)([-*+])\s+(.*)$/;
const NUMBERED = /^(\s*)(\d{1,9})[.)]\s+(.*)$/;
const TABLE_SEP = /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/;

// ------------------------------------------------------------------- inline

function isWordChar(ch: string | undefined): boolean {
  return ch !== undefined && /[\p{L}\p{N}]/u.test(ch);
}

/** Parse inline Markdown. `*`/`_` only emphasise when they open and close around real text. */
export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let text = '';
  const flush = () => {
    if (text) out.push({ t: 'text', v: text });
    text = '';
  };
  let i = 0;
  while (i < src.length) {
    const ch = src[i];

    if (ch === '\\' && i + 1 < src.length && /[\\`*_{}[\]()#+\-.!~|>]/.test(src[i + 1])) {
      text += src[i + 1];
      i += 2;
      continue;
    }

    if (ch === '`') {
      let run = 1;
      while (src[i + run] === '`') run++;
      const fence = '`'.repeat(run);
      const end = src.indexOf(fence, i + run);
      if (end !== -1) {
        flush();
        out.push({ t: 'code', v: src.slice(i + run, end).replace(/^ (.*) $/s, '$1') });
        i = end + run;
        continue;
      }
      text += fence;
      i += run;
      continue;
    }

    if (ch === '[') {
      const m = /^\[([^\]]*)\]\(\s*((?:[^()\s]|\([^()\s]*\))+)(?:\s+"[^"]*")?\s*\)/.exec(src.slice(i));
      if (m) {
        flush();
        const inner = parseInline(m[1]);
        out.push(SAFE_HREF.test(m[2]) ? { t: 'link', href: m[2], c: inner } : { t: 'text', v: m[1] });
        i += m[0].length;
        continue;
      }
    }

    if ((ch === 'h' || ch === 'H') && !isWordChar(src[i - 1])) {
      const m = BARE_URL.exec(src.slice(i));
      if (m) {
        flush();
        out.push({ t: 'link', href: m[0], c: [{ t: 'text', v: m[0] }] });
        i += m[0].length;
        continue;
      }
    }

    if (ch === '*' || ch === '_' || ch === '~') {
      const double = src[i + 1] === ch;
      const mark = ch === '~' ? '~~' : double ? ch + ch : ch;
      if (ch !== '~' || double) {
        const open = i + mark.length;
        const before = src[i - 1];
        const canOpen = src[open] !== undefined && !/\s/.test(src[open]) && !(ch === '_' && isWordChar(before));
        if (canOpen) {
          let end = src.indexOf(mark, open);
          // Skip closers that sit after whitespace, or inside a word for `_`.
          while (end !== -1 && (/\s/.test(src[end - 1]) || (ch === '_' && isWordChar(src[end + mark.length])))) {
            end = src.indexOf(mark, end + 1);
          }
          if (end > open) {
            // `***x***`: the closing run is longer than the mark, so the mark is its last part.
            let run = 0;
            while (src[end + run] === ch) run++;
            if (ch !== '~' && run > mark.length) end += run - mark.length;
            flush();
            const c = parseInline(src.slice(open, end));
            out.push(ch === '~' ? { t: 'del', c } : mark.length === 2 ? { t: 'strong', c } : { t: 'em', c });
            i = end + mark.length;
            continue;
          }
        }
      }
    }

    text += ch;
    i++;
  }
  flush();
  return out;
}

/** A paragraph's lines joined, with a line break at each newline (matches what the terminal showed). */
function inlineLines(lines: string[]): Inline[] {
  const out: Inline[] = [];
  lines.forEach((l, k) => {
    if (k > 0) out.push({ t: 'br' });
    out.push(...parseInline(l.replace(/\s+$/, '')));
  });
  return out;
}

// -------------------------------------------------------------------- blocks

function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  return s.split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, '|'));
}

function indentOf(line: string): number {
  return line.length - line.trimStart().length;
}

function startsBlock(line: string, next: string | undefined): boolean {
  return (
    FENCE.test(line) ||
    HEADING.test(line) ||
    RULE.test(line) ||
    /^\s*>/.test(line) ||
    BULLET.test(line) ||
    NUMBERED.test(line) ||
    (line.includes('|') && next !== undefined && TABLE_SEP.test(next) && next.includes('-'))
  );
}

export function parseMarkdown(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.trim() === '') {
      i++;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !new RegExp(`^ {0,3}${marker[0]}{${marker.length},}\\s*$`).test(lines[i])) {
        body.push(lines[i]);
        i++;
      }
      i++; // the closing fence (or the end of input: an unterminated block is still shown)
      out.push({ t: 'code', lang: fence[2], v: body.join('\n') });
      continue;
    }

    const h = HEADING.exec(line);
    if (h) {
      out.push({ t: 'h', level: h[1].length, c: parseInline(h[2]) });
      i++;
      continue;
    }

    if (RULE.test(line)) {
      out.push({ t: 'hr' });
      i++;
      continue;
    }

    if (/^\s*>/.test(line)) {
      const quoted: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) {
        quoted.push(lines[i].replace(/^\s*> ?/, ''));
        i++;
      }
      out.push({ t: 'quote', c: parseMarkdown(quoted.join('\n')) });
      continue;
    }

    if (line.includes('|') && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1]) && lines[i + 1].includes('-')) {
      const head = splitRow(line).map(parseInline);
      i += 2;
      const rows: Inline[][][] = [];
      while (i < lines.length && lines[i].trim() !== '' && lines[i].includes('|')) {
        rows.push(splitRow(lines[i]).map(parseInline));
        i++;
      }
      out.push({ t: 'table', head, rows });
      continue;
    }

    const bullet = BULLET.exec(line);
    const numbered = bullet ? null : NUMBERED.exec(line);
    if (bullet || numbered) {
      const m = (bullet ?? numbered) as RegExpExecArray;
      const ordered = !bullet;
      const base = m[1].length;
      const items: Block[][] = [];
      const start = ordered ? Number(m[2]) : 1;
      while (i < lines.length) {
        const cur = lines[i];
        const im = (ordered ? NUMBERED : BULLET).exec(cur);
        if (!im || im[1].length !== base) break;
        // The item's text, plus every following line indented deeper (or a blank before one).
        const body = [im[3]];
        const contentCol = cur.length - im[3].length; // where the item's text starts
        i++;
        while (i < lines.length) {
          const next = lines[i];
          if (next.trim() === '') {
            const after = lines[i + 1];
            if (after !== undefined && after.trim() !== '' && indentOf(after) > base) {
              body.push('');
              i++;
              continue;
            }
            break;
          }
          if (indentOf(next) > base) {
            body.push(next.slice(Math.min(indentOf(next), contentCol)));
            i++;
            continue;
          }
          break;
        }
        items.push(parseMarkdown(body.join('\n')));
        // A blank line between items keeps the list going.
        if (i < lines.length && lines[i].trim() === '') {
          const after = lines[i + 1];
          if (after !== undefined && (ordered ? NUMBERED : BULLET).test(after) && indentOf(after) === base) i++;
        }
      }
      out.push({ t: 'list', ordered, start, items });
      continue;
    }

    // A paragraph runs until a blank line or the start of another block.
    const para: string[] = [line];
    i++;
    while (i < lines.length && lines[i].trim() !== '' && !startsBlock(lines[i], lines[i + 1])) {
      para.push(lines[i]);
      i++;
    }
    out.push({ t: 'p', c: inlineLines(para) });
  }
  return out;
}

// ------------------------------------------------------------------ preview

/** Flatten blocks into lines of inline content: headings bold, list items bulleted, code on one line. */
function previewLines(blocks: Block[], out: Inline[][]) {
  for (const b of blocks) {
    switch (b.t) {
      case 'p':
        out.push(b.c);
        break;
      case 'h':
        out.push([{ t: 'strong', c: b.c }]);
        break;
      case 'code':
        out.push([{ t: 'code', v: b.v.trim().replace(/\s*\n\s*/g, ' ') }]);
        break;
      case 'quote':
        previewLines(b.c, out);
        break;
      case 'list':
        b.items.forEach((item, i) => {
          const start = out.length;
          previewLines(item, out);
          const bullet: Inline = { t: 'text', v: b.ordered ? `${b.start + i}. ` : '• ' };
          if (out.length === start) out.push([bullet]);
          else out[start] = [bullet, ...out[start]!];
        });
        break;
      case 'table':
        out.push([{ t: 'text', v: b.head.map((c) => inlineText(c)).join(' · ') }]);
        break;
      case 'hr':
        break;
    }
  }
}

/** The plain text of some inline content. */
function inlineText(nodes: Inline[]): string {
  return nodes
    .map((n) => (n.t === 'text' || n.t === 'code' ? n.v : n.t === 'br' ? ' ' : inlineText(n.c)))
    .join('');
}

/** Links become their text: a preview sits inside a button, so it cannot hold a link. */
function unlink(nodes: Inline[]): Inline[] {
  return nodes.flatMap((n): Inline[] => {
    if (n.t === 'link') return unlink(n.c);
    if (n.t === 'strong' || n.t === 'em' || n.t === 'del') return [{ ...n, c: unlink(n.c) }];
    return [n];
  });
}

/** A short message as inline Markdown for a one-or-two-line preview: no blocks, no links, a line break between lines. */
export function previewInlines(src: string): Inline[] {
  const lines: Inline[][] = [];
  previewLines(parseMarkdown(src), lines);
  return unlink(lines.flatMap((l, i) => (i === 0 ? l : [{ t: 'br' } as Inline, ...l])));
}
