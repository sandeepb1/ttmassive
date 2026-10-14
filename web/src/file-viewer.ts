// A read-only view of a file a terminal printed the path of: line numbers, light syntax colours
// and the line that was named, scrolled into view. The server only serves files inside the folder
// the session works in (see src/files.rs), so a refusal shows its reason here.

import { el } from './dom';
import { highlight, languageFor, lineHtml } from './highlight';
import { Sheet } from './sheet';

interface FileView {
  path: string;
  rel: string;
  name: string;
  size: number;
  truncated: boolean;
  lines: number;
  text: string;
}

/** Lines past this are not drawn: it keeps the page quick, and the server stops at 1 MB anyway. */
const MAX_LINES = 20_000;
const PAD = 8; // px, above the first line

export function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export class FileViewer extends Sheet {
  constructor(
    host: HTMLElement,
    session: string,
    private file: string,
    private line: number | null,
    private signedOut: () => void,
  ) {
    super(host, session, 'File', signedOut);
    this.tall();
    this.wide();
  }

  async open() {
    this.message('Loading…');
    this.show();
    let r: Response;
    try {
      r = await fetch(`/api/sessions/${encodeURIComponent(this.session)}/file?path=${encodeURIComponent(this.file)}`, { cache: 'no-store' });
    } catch {
      return this.message('Server unreachable', true);
    }
    if (r.status === 401) return this.signedOut();
    if (!r.ok) return this.message((await r.text().catch(() => '')) || 'Could not open that file', true);
    this.render((await r.json()) as FileView);
  }

  private render(v: FileView) {
    if (this.gone) return;
    const root = el('div', 'fv');

    const head = el('div', 'fv-head');
    const path = el('span', 'fv-path', v.rel);
    path.title = v.path;
    const meta = el('span', 'fv-meta', `${v.lines} line${v.lines === 1 ? '' : 's'} · ${fmtSize(v.size)}`);
    const copy = el('button', 'chip', 'Copy path');
    copy.type = 'button';
    copy.addEventListener('click', () => {
      void navigator.clipboard.writeText(v.path).then(
        () => (copy.textContent = 'Copied'),
        () => (copy.textContent = 'Blocked'),
      );
    });
    head.append(path, meta, copy);
    root.append(head);

    const notes: string[] = [];
    if (v.truncated) notes.push(`Only the first ${fmtSize(v.text.length)} of this ${fmtSize(v.size)} file is shown.`);
    if (this.line !== null && this.line > v.lines) notes.push(`Line ${this.line} is past the end: the file has ${v.lines}.`);
    if (v.lines > MAX_LINES) notes.push(`Showing the first ${MAX_LINES} lines.`);
    if (notes.length) root.append(el('div', 'fv-note', notes.join(' ')));

    const tokens = highlight(v.text, languageFor(v.name)).slice(0, MAX_LINES);
    const scroller = el('div', 'fv-scroll');
    const inner = el('div', 'fv-inner');
    const gutter = el('pre', 'fv-gutter');
    gutter.setAttribute('aria-hidden', 'true');
    gutter.textContent = tokens.map((_, i) => String(i + 1)).join('\n');
    const code = el('pre', 'fv-code');
    code.innerHTML = tokens.map(lineHtml).join('\n'); // escaped by lineHtml

    const target = this.line !== null && this.line >= 1 && this.line <= tokens.length ? this.line : null;
    if (target !== null) {
      const mark = el('div', 'fv-mark');
      mark.style.top = `calc(${PAD}px + ${target - 1} * var(--fv-lh))`;
      inner.append(mark); // first, so the text paints over it
    }
    inner.append(gutter, code);
    scroller.append(inner);
    root.append(scroller);
    this.body.replaceChildren(root);

    if (target !== null) {
      requestAnimationFrame(() => {
        const lh = parseFloat(getComputedStyle(scroller).getPropertyValue('--fv-lh')) || 20;
        scroller.scrollTop = Math.max(0, PAD + (target - 1) * lh - scroller.clientHeight / 3);
      });
    }
  }
}
