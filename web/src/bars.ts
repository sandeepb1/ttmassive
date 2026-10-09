// Two small bars above the key bar: answer buttons for a question on screen, and
// the restore actions for a stopped session.

import type { Analysis } from './protocol';

function btn(label: string, cls: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = cls;
  b.textContent = label;
  // Don't steal focus from the terminal (keeps the soft keyboard up).
  b.addEventListener('pointerdown', (e) => e.preventDefault());
  b.addEventListener('click', onClick);
  return b;
}

function clip(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

/** Shows the agent's question with one button per answer, or an Interrupt button while it works. */
export class PromptBar {
  private key = '';
  private busy = false;

  constructor(
    private el: HTMLElement,
    private answer: (choice: string, promptId: string | null) => Promise<string | null>,
  ) {
    el.hidden = true;
  }

  render(a: Analysis | null) {
    const p = a?.prompt ?? null;
    const working = a?.activity === 'working';
    const key = p ? `p:${p.id}` : working ? 'w' : '';
    if (key === this.key) return;
    this.key = key;
    if (!key) {
      this.el.hidden = true;
      this.el.replaceChildren();
      return;
    }
    this.el.hidden = false;
    if (!p) {
      this.el.replaceChildren(
        this.label('working…'),
        btn('Interrupt', 'pb-btn', () => void this.send('interrupt', null)),
      );
      return;
    }
    const q = document.createElement('div');
    q.className = 'pb-q';
    const ctx = p.context.at(-1);
    q.textContent = clip(ctx ? `${ctx}  ·  ${p.question}` : p.question, 140);
    const row = document.createElement('div');
    row.className = 'pb-row';
    for (const c of p.choices.slice(0, 5)) {
      row.append(
        btn(`${c.index}. ${clip(c.label, 38)}`, c.selected ? 'pb-btn pb-default' : 'pb-btn', () =>
          void this.send(String(c.index), p.id),
        ),
      );
    }
    this.el.replaceChildren(q, row);
  }

  private label(text: string): HTMLElement {
    const s = document.createElement('span');
    s.className = 'pb-label';
    s.textContent = text;
    return s;
  }

  private async send(choice: string, promptId: string | null) {
    if (this.busy) return;
    this.busy = true;
    this.el.classList.add('busy');
    const err = await this.answer(choice, promptId);
    this.busy = false;
    this.el.classList.remove('busy');
    if (err) {
      this.key = '';
      const e = document.createElement('div');
      e.className = 'pb-err';
      e.textContent = err;
      this.el.append(e);
    }
  }
}

/** Offered when a session only exists as saved output. */
export class StoppedBar {
  constructor(
    private el: HTMLElement,
    private restore: (fresh: boolean) => Promise<string | null>,
  ) {
    el.hidden = true;
  }

  hide() {
    this.el.hidden = true;
    this.el.replaceChildren();
  }

  show(resumable: boolean) {
    const msg = document.createElement('span');
    msg.className = 'pb-label';
    msg.textContent = 'Session stopped. Showing its last saved screen.';
    const err = document.createElement('div');
    err.className = 'pb-err';
    const act = async (fresh: boolean) => {
      const e = await this.restore(fresh);
      if (e) err.textContent = e;
    };
    const row = document.createElement('div');
    row.className = 'pb-row';
    if (resumable) row.append(btn('Resume conversation', 'pb-btn pb-default', () => void act(false)));
    row.append(btn(resumable ? 'Start fresh' : 'Restore', 'pb-btn', () => void act(true)));
    this.el.hidden = false;
    this.el.replaceChildren(msg, row, err);
  }
}
