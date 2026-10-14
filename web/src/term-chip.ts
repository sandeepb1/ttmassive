// The corner chip on a terminal pane, drawn from the logic in `activity-chip.ts`: a coloured dot,
// what the agent is doing and how long. It fades out when the agent is idle, and ignores taps so
// it never gets between you and the terminal.

import { chip, chipText, observe, type Activity, type Tracker, type Work } from './activity-chip';
import { el } from './dom';

export class ActivityChip {
  private root = el('div', 't-chip');
  private text = el('span', 't-chip-text');
  private tracker: Tracker | null = null;
  private work: Work = { doing: null, turnStart: null };
  private skew = 0;
  private timer = 0;

  constructor(host: HTMLElement) {
    this.root.setAttribute('aria-hidden', 'true'); // the pane header already announces the status
    this.root.append(el('span', 't-chip-dot'), this.text);
    host.append(this.root);
  }

  /** What the screen reader says now, and how far the server's clock is ahead of this page's. */
  observe(activity: Activity, skewMs: number) {
    this.skew = skewMs;
    this.tracker = observe(this.tracker, activity, Date.now());
    this.draw();
  }

  /** What the conversation says the agent is doing, and when your message was sent. */
  setWork(work: Work) {
    this.work = work;
    this.draw();
  }

  /** The pane shows another session: forget this one. */
  reset() {
    this.tracker = null;
    this.work = { doing: null, turnStart: null };
    this.draw();
  }

  dispose() {
    window.clearInterval(this.timer);
    this.root.remove();
  }

  private draw() {
    const c = chip(this.tracker, this.work, this.skew, Date.now());
    if (!c) {
      this.root.classList.remove('show');
      window.clearInterval(this.timer);
      this.timer = 0;
      return;
    }
    this.root.dataset.kind = c.kind;
    this.text.textContent = chipText(c);
    this.root.classList.add('show');
    // The elapsed time moves every second; the screen reading only every two.
    if (!this.timer) this.timer = window.setInterval(() => this.draw(), 1000);
  }
}
