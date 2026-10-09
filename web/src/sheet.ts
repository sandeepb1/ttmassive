// A bottom sheet over the chat, and the plumbing the toggle sheets share: open and close it,
// show a message in it, and call the session's API.

import { el } from './dom';
import { dragToDismiss } from './gestures';

export type CallResult<T> = { ok: true; data: T } | { ok: false; error: string };

export abstract class Sheet {
  private backdrop = el('div', 'sheet-backdrop');
  private sheet = el('div', 'sheet');
  protected body = el('div', 'sheet-body');
  protected gone = true;
  protected busy = false;

  constructor(
    protected host: HTMLElement,
    protected session: string,
    title: string,
    private onUnauthorized: () => void,
  ) {
    this.backdrop.addEventListener('click', () => this.close());
    // A tap on a button must not take focus from the message box: that would drop the on-screen
    // keyboard and make the sheet jump under the next tap.
    this.sheet.addEventListener('pointerdown', (e) => {
      if (e.target instanceof Element && e.target.closest('button')) e.preventDefault();
    });
    const head = el('div', 'sheet-head');
    head.append(el('strong', undefined, title));
    const x = el('button', 'sheet-x', '×');
    x.type = 'button';
    x.setAttribute('aria-label', 'Close');
    x.addEventListener('click', () => this.close());
    head.append(x);
    this.sheet.append(head, this.body);
    dragToDismiss(this.sheet, head, () => {
      this.sheet.style.transform = '';
      this.close();
    });
  }

  /** Put the sheet on screen. */
  protected show() {
    this.gone = false;
    this.host.append(this.backdrop, this.sheet);
  }

  close() {
    if (this.gone) return;
    this.gone = true;
    this.backdrop.remove();
    this.sheet.remove();
    this.onClose();
  }

  /** Runs once when the sheet is closed, whichever way. */
  protected onClose() {}

  protected message(text: string, isError = false) {
    this.body.replaceChildren(el('div', isError ? 'sheet-msg err' : 'sheet-msg', text));
  }

  /** POST to `/api/sessions/{id}/{path}`; failures come back as a message to show. */
  protected async call<T>(path: string, body?: unknown): Promise<CallResult<T>> {
    const r = await fetch(`/api/sessions/${encodeURIComponent(this.session)}/${path}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    }).catch(() => null);
    if (!r) return { ok: false, error: 'Server unreachable' };
    if (r.status === 401) {
      this.onUnauthorized();
      return { ok: false, error: 'Signed out' };
    }
    if (!r.ok) return { ok: false, error: (await r.text()) || 'That did not work' };
    const text = await r.text();
    return { ok: true, data: (text ? JSON.parse(text) : {}) as T };
  }
}

/** A row button with a title, an optional right-hand mark and a description. */
export function row(label: string, opts: { hint?: string; mark?: string; cls?: string } = {}): HTMLButtonElement {
  const b = el('button', ['sheet-row', opts.cls].filter(Boolean).join(' '));
  b.type = 'button';
  const top = el('span', 'sr-top');
  top.append(el('span', 'sr-label', label));
  if (opts.mark) top.append(el('span', 'sr-mark', opts.mark));
  b.append(top);
  if (opts.hint) b.append(el('span', 'sr-desc', opts.hint));
  return b;
}
