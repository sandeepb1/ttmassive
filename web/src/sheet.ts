// A bottom sheet over the chat, and the plumbing the toggle sheets share: open and close it,
// show a message in it, and call the session's API.

import { el } from './dom';
import { sheetShell, type SheetShell } from './ui/sheet';
import { sheetMsg } from './ui/sheet-parts';

export type CallResult<T> = { ok: true; data: T } | { ok: false; error: string };

export abstract class Sheet {
  private shell: SheetShell;
  protected body: HTMLElement;
  protected gone = true;
  protected busy = false;

  constructor(
    protected host: HTMLElement,
    protected session: string,
    title: string,
    private onUnauthorized: () => void,
  ) {
    this.shell = sheetShell(title, () => this.close());
    this.body = this.shell.body;
  }

  /** Make the sheet nearly full height, for content that scrolls on its own. */
  protected tall() {
    this.shell.tall();
  }

  /** Let the sheet use the width of a desktop screen, for content that is wide, like code. */
  protected wide() {
    this.shell.wide();
  }

  /** Put the sheet on screen. */
  protected show() {
    this.gone = false;
    this.shell.open(this.host);
  }

  close() {
    if (this.gone) return;
    this.gone = true;
    this.shell.remove();
    this.onClose();
  }

  /** Runs once when the sheet is closed, whichever way. */
  protected onClose() {}

  protected message(text: string, isError = false) {
    this.body.replaceChildren(sheetMsg(text, isError));
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

export { row } from './ui/row';
