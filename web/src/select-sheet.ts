// "Select text": the terminal is drawn on a canvas, so a phone has no text to long-press. This sheet
// shows the screen and recent history as real text, where the system's own selection handles,
// magnifier and Copy menu work.

import { el } from './dom';
import { Sheet } from './sheet';

const LINES = 2000;

export class SelectSheet extends Sheet {
  constructor(
    host: HTMLElement,
    session: string,
    private signedOut: () => void,
  ) {
    super(host, session, 'Select text', signedOut);
    this.tall();
    this.wide();
  }

  async open() {
    this.message('Loading…');
    this.show();
    let r: Response;
    try {
      r = await fetch(`/api/sessions/${encodeURIComponent(this.session)}/output?lines=${LINES}`, { cache: 'no-store' });
    } catch {
      return this.message('Server unreachable', true);
    }
    if (r.status === 401) return this.signedOut();
    if (!r.ok) return this.message((await r.text().catch(() => '')) || 'Could not read the terminal', true);
    this.render(await r.text());
  }

  private render(text: string) {
    if (this.gone) return;
    const pre = el('pre', 'sel-text');
    pre.textContent = text;
    pre.tabIndex = 0;

    const all = el('button', 'chip', 'Copy all');
    all.type = 'button';
    all.addEventListener('click', () => {
      navigator.clipboard.writeText(text).then(
        () => (all.textContent = 'Copied'),
        () => (all.textContent = 'Blocked'),
      );
    });
    const bar = el('div', 'sel-bar');
    bar.append(el('span', 'sheet-hint', 'Touch and hold a word, drag the handles, then Copy.'), all);

    this.body.replaceChildren(bar, pre);
    // Start at the newest output, which is where the screen is.
    pre.scrollTop = pre.scrollHeight;
  }
}
