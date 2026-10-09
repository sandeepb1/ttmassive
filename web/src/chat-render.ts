// Turns chat items into DOM. Everything the agent wrote goes in with `textContent` or a text
// node, never `innerHTML`, so agent output cannot inject markup.

import type { ChatItem, DiffData, Question } from './chat-model';
import { initialForm, isComplete, isInstant, pickOption, toRequest, typeAnswer, type QuestionRequest } from './question-form';
import { splitUploads } from './composer-text';
import { lineDiff, trimContext } from './diff';
import { el } from './dom';
import { icon, toolCategory, toolIconName } from './icons';
import { parseMarkdown, type Block, type Inline } from './markdown';

// ----------------------------------------------------------------- markdown

function inline(nodes: Inline[]): Node[] {
  return nodes.map((n): Node => {
    switch (n.t) {
      case 'text':
        return document.createTextNode(n.v);
      case 'code':
        return el('code', 'md-code', n.v);
      case 'strong': {
        const e = el('strong');
        e.append(...inline(n.c));
        return e;
      }
      case 'em': {
        const e = el('em');
        e.append(...inline(n.c));
        return e;
      }
      case 'del': {
        const e = el('del');
        e.append(...inline(n.c));
        return e;
      }
      case 'link': {
        const a = el('a');
        a.href = n.href; // the parser only lets http(s) and mailto through
        a.target = '_blank';
        a.rel = 'noopener noreferrer';
        a.append(...inline(n.c));
        return a;
      }
      case 'br':
        return el('br');
    }
  });
}

/** A code block with a Copy button. */
function codeBlock(lang: string, text: string): HTMLElement {
  const box = el('div', 'codeblock');
  const head = el('div', 'code-head');
  head.append(el('span', 'code-lang', lang || 'text'));
  const copy = el('button', 'code-copy', 'Copy');
  copy.type = 'button';
  copy.addEventListener('click', () => {
    void navigator.clipboard?.writeText(text).then(
      () => {
        copy.textContent = 'Copied';
        window.setTimeout(() => (copy.textContent = 'Copy'), 1200);
      },
      () => (copy.textContent = 'Not allowed'),
    );
  });
  head.append(copy);
  const pre = el('pre');
  pre.append(el('code', undefined, text));
  box.append(head, pre);
  return box;
}

function blocks(bs: Block[]): HTMLElement[] {
  return bs.map((b): HTMLElement => {
    switch (b.t) {
      case 'p': {
        const p = el('p');
        p.append(...inline(b.c));
        return p;
      }
      case 'h': {
        const h = el(`h${b.level}` as 'h1');
        h.append(...inline(b.c));
        return h;
      }
      case 'code':
        return codeBlock(b.lang, b.v);
      case 'hr':
        return el('hr');
      case 'quote': {
        const q = el('blockquote');
        q.append(...blocks(b.c));
        return q;
      }
      case 'list': {
        const list: HTMLOListElement | HTMLUListElement = b.ordered ? el('ol') : el('ul');
        if (list instanceof HTMLOListElement && b.start !== 1) list.start = b.start;
        for (const item of b.items) {
          const li = el('li');
          // A one-paragraph item reads better without the paragraph's margins.
          if (item.length === 1 && item[0].t === 'p') li.append(...inline(item[0].c));
          else li.append(...blocks(item));
          list.append(li);
        }
        return list;
      }
      case 'table': {
        const wrap = el('div', 'md-table');
        const t = el('table');
        const tr = el('tr');
        for (const c of b.head) {
          const th = el('th');
          th.append(...inline(c));
          tr.append(th);
        }
        const thead = el('thead');
        thead.append(tr);
        t.append(thead);
        const body = el('tbody');
        for (const r of b.rows) {
          const row = el('tr');
          for (const c of r) {
            const td = el('td');
            td.append(...inline(c));
            row.append(td);
          }
          body.append(row);
        }
        t.append(body);
        wrap.append(t);
        return wrap;
      }
    }
  });
}

export function renderMarkdown(src: string): HTMLElement {
  const box = el('div', 'md');
  box.append(...blocks(parseMarkdown(src)));
  return box;
}

// -------------------------------------------------------------------- diffs

export function renderDiff(d: DiffData): HTMLElement {
  const box = el('div', 'diff');
  box.append(el('div', 'diff-path', d.path));
  d.edits.forEach((e, k) => {
    if (k > 0) box.append(el('div', 'diff-gap', '· · ·'));
    for (const l of trimContext(lineDiff(e.old, e.new), 2)) {
      if (l.t === 'gap') {
        box.append(el('div', 'diff-gap', `⋯ ${l.n} unchanged line${l.n === 1 ? '' : 's'}`));
      } else {
        const sign = l.t === 'add' ? '+' : l.t === 'del' ? '−' : ' ';
        box.append(el('div', `diff-line ${l.t}`, `${sign} ${l.v}`));
      }
    }
  });
  return box;
}

// -------------------------------------------------------------------- items

const STATUS_MARK = { running: '…', ok: '✓', error: '✕' } as const;

function pre(label: string, text: string): HTMLElement {
  const box = el('div', 't-block');
  box.append(el('div', 't-label', label), el('pre', undefined, text));
  return box;
}

function renderTool(it: ChatItem & { kind: 'tool' }): HTMLElement {
  const cat = toolCategory(it.tool);
  const d = el('details', `tool st-${it.status} cat-${cat}`);
  const sum = el('summary');
  const badge = el('span', 't-icon');
  badge.append(icon(toolIconName(cat)));
  sum.append(
    badge,
    el('span', 't-name', it.tool),
    el('span', 't-sum', it.summary),
    el('span', 't-mark', STATUS_MARK[it.status]),
  );
  d.append(sum);
  const body = el('div', 't-body');
  if (it.diff) body.append(renderDiff(it.diff));
  else if (it.input) body.append(pre('Input', it.input));
  if (it.output && !(it.diff && it.status !== 'error')) body.append(pre(it.status === 'error' ? 'Error' : 'Output', it.output));
  if (body.childElementCount > 0) d.append(body);
  else d.classList.add('t-empty'); // nothing to expand
  return d;
}

/** Whether `answer` (e.g. `Nuts, Cream`) chose the option called `label`. */
function chose(answer: string | undefined, label: string, multi: boolean): boolean {
  if (answer === undefined) return false;
  return multi ? answer.split(',').map((s) => s.trim()).includes(label) : answer === label;
}

/** What the view lets a rendered item do. Without it, questions are shown read-only. */
export interface RenderCtx {
  /** Send the answers. Resolves with an error message, or null once they were typed in. */
  answerQuestion: (req: QuestionRequest) => Promise<string | null>;
  dismissQuestion: () => Promise<string | null>;
}

/** The agent is waiting: options to tap, a box for your own answer, and Send. */
function liveQuestions(it: ChatItem & { kind: 'question' }, ctx: RenderCtx): HTMLElement {
  const qs = it.questions;
  let state = initialForm(qs);
  let busy = false;
  const card = el('div', 'qcard q-pending q-live');
  const optionButtons: HTMLButtonElement[][] = [];
  const inputs: (HTMLInputElement | null)[] = [];
  const send = el('button', 'q-send', 'Send');
  const dismiss = el('button', 'q-dismiss', 'Dismiss');
  const msg = el('div', 'q-err');
  send.type = 'button';
  dismiss.type = 'button';

  const sync = () => {
    optionButtons.forEach((row, i) => row.forEach((b, k) => b.classList.toggle('picked', state[i].choices.includes(k + 1))));
    inputs.forEach((inp, i) => {
      if (inp && inp.value !== state[i].text) inp.value = state[i].text;
    });
    send.disabled = busy || !isComplete(state);
    dismiss.disabled = busy;
    card.classList.toggle('busy', busy);
  };
  const run = async (action: () => Promise<string | null>) => {
    busy = true;
    msg.textContent = '';
    sync();
    const err = await action();
    if (err) msg.textContent = err;
    // On success the log marks the question answered and this card is replaced. If that never
    // happens, hand control back after a while rather than leaving the card dead.
    window.setTimeout(() => {
      busy = false;
      sync();
    }, err ? 0 : 12_000);
  };
  const submit = () => {
    if (!busy && isComplete(state)) void run(() => ctx.answerQuestion(toRequest(state)));
  };

  qs.forEach((q, i) => {
    const box = el('div', 'q');
    if (q.header) box.append(el('span', 'q-chip', q.header));
    box.append(el('div', 'q-text', q.question));
    if (q.multi_select) box.append(el('div', 'q-hint', 'Choose all that apply'));
    const row: HTMLButtonElement[] = [];
    const opts = el('div', 'q-opts');
    q.options.forEach((o, k) => {
      const b = el('button', 'q-opt q-opt-btn');
      b.type = 'button';
      b.append(el('span', 'q-label', o.label));
      if (o.description) b.append(el('span', 'q-desc', o.description));
      b.addEventListener('click', () => {
        if (busy) return;
        state = pickOption(qs, state, i, k + 1);
        sync();
        if (isInstant(qs)) submit(); // one question, one answer: tapping it is the answer
      });
      row.push(b);
      opts.append(b);
    });
    optionButtons.push(row);
    box.append(opts);
    if (q.multi_select) {
      inputs.push(null); // typed answers are not available on multiple choice
    } else {
      const input = el('input', 'q-other');
      input.type = 'text';
      input.placeholder = 'Or type your own answer';
      input.enterKeyHint = 'send';
      input.addEventListener('input', () => {
        state = typeAnswer(state, i, input.value);
        sync();
      });
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && !e.isComposing) {
          e.preventDefault();
          submit();
        }
      });
      inputs.push(input);
      box.append(input);
    }
    card.append(box);
  });

  send.addEventListener('click', submit);
  dismiss.addEventListener('click', () => void run(() => ctx.dismissQuestion()));
  const actions = el('div', 'q-actions');
  actions.append(dismiss, send);
  card.append(msg, actions);
  sync();
  return card;
}

function renderQuestions(it: ChatItem & { kind: 'question' }, ctx?: RenderCtx): HTMLElement {
  if (it.state === 'pending' && ctx) return liveQuestions(it, ctx);
  const card = el('div', `qcard q-${it.state}`);
  for (const q of it.questions) {
    const a = it.answers.find((x) => x.question === q.question)?.answer;
    card.append(renderOneQuestion(q, a));
  }
  const note =
    it.state === 'pending' ? 'Waiting for your answer' : it.state === 'cancelled' ? 'Dismissed without an answer' : '';
  if (note) card.append(el('div', 'q-note', note));
  return card;
}

function renderOneQuestion(q: Question, answer: string | undefined): HTMLElement {
  const box = el('div', 'q');
  if (q.header) box.append(el('span', 'q-chip', q.header));
  box.append(el('div', 'q-text', q.question));
  const opts = el('div', 'q-opts');
  for (const o of q.options) {
    const row = el('div', chose(answer, o.label, q.multi_select) ? 'q-opt picked' : 'q-opt');
    row.append(el('span', 'q-label', o.label));
    if (o.description) row.append(el('span', 'q-desc', o.description));
    opts.append(row);
  }
  box.append(opts);
  // A typed-in answer is not one of the options: show it on its own.
  if (answer !== undefined && !q.options.some((o) => chose(answer, o.label, q.multi_select))) {
    box.append(el('div', 'q-typed', `Answered: ${answer}`));
  }
  return box;
}

/** Local time of day for a Unix-seconds timestamp. */
export function clock(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

/** The first line of a thought, short enough to hint at it while it is collapsed. */
function preview(text: string): string {
  const line = text.split('\n').find((l) => l.trim()) ?? '';
  return line.length > 80 ? line.slice(0, 79) + '…' : line;
}

/** One chat item as DOM. Items are re-rendered whole whenever they change. */
export function renderItem(it: ChatItem, ctx?: RenderCtx): HTMLElement {
  switch (it.kind) {
    case 'user': {
      const m = el('div', 'msg user');
      // Images attached through ttm are written into the message as `@path`: show them as pictures.
      const { text, images } = splitUploads(it.text);
      for (const url of images) {
        const img = el('img', 'msg-img');
        img.src = url;
        img.alt = 'Attached image';
        img.loading = 'lazy';
        m.append(img);
      }
      if (text) m.append(el('div', 'bubble', text));
      if (it.images > 0) m.append(el('div', 'chip-img', `🖼 ${it.images} image${it.images === 1 ? '' : 's'}`));
      if (it.ts !== null) {
        const t = el('time', 'msg-time', clock(it.ts));
        t.dateTime = new Date(it.ts * 1000).toISOString();
        m.append(t);
      }
      return m;
    }
    case 'assistant': {
      const m = el('div', 'msg assistant');
      m.append(renderMarkdown(it.text));
      return m;
    }
    case 'thinking': {
      const d = el('details', 'thought');
      const sum = el('summary');
      sum.append(icon('chevron', 'icon-svg chev'), el('span', 'th-label', 'Thinking'), el('span', 'th-preview', preview(it.text)));
      d.append(sum, el('div', 't-text', it.text));
      return d;
    }
    case 'command':
      return el('div', 'msg command', it.text);
    case 'notice':
      return el('div', 'msg notice', it.text);
    case 'tool':
      return renderTool(it);
    case 'question':
      return renderQuestions(it, ctx);
  }
}
