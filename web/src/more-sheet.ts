// The "More" sheet: reasoning up and down, what Codex may do, one-tap commands, and reusing your
// last message. Anything that changes how much the agent may do unasked, or throws away the
// conversation, asks for a second tap first.

import { stepEffort } from './chat-model';
import { el } from './dom';
import { warn } from './haptics';
import { Sheet, row } from './sheet';

interface PermRow {
  number: number;
  label: string;
  description: string;
  current: boolean;
}

export interface MoreState {
  agent: 'claude' | 'codex';
  /** The effort level now, as the chip shows it. */
  effort: string | null;
  /** There is a message of yours to edit. */
  hasLast: boolean;
}

export interface MoreHooks {
  onUnauthorized: () => void;
  /** The reasoning level changed. */
  onEffort: (level: string) => void;
  /** Say something short above the composer. */
  onSay: (text: string) => void;
  /** Copy your last message into the composer. */
  onReuse: () => void;
}

export class MoreSheet extends Sheet {
  private state: MoreState = { agent: 'claude', effort: null, hasLast: false };
  /** Codex's permissions menu is open on the host and still needs closing. */
  private permsOpen = false;
  private permRows: PermRow[] = [];

  constructor(
    host: HTMLElement,
    session: string,
    private hooks: MoreHooks,
  ) {
    super(host, session, 'More', hooks.onUnauthorized);
  }

  open(state: MoreState) {
    if (!this.gone) return;
    this.state = state;
    this.show();
    this.renderMain();
  }

  protected override onClose() {
    if (this.permsOpen) {
      this.permsOpen = false;
      void this.call('model/close');
    }
  }

  // ----------------------------------------------------------------- main

  private renderMain(note = '') {
    const { agent, effort, hasLast } = this.state;
    const parts: HTMLElement[] = [];
    if (note) parts.push(el('div', 'sheet-hint', note));

    // Reasoning
    const reasoning = el('div', 'more-block');
    reasoning.append(el('div', 'sheet-label', 'Reasoning'));
    const stepper = el('div', 'stepper');
    const less = el('button', 'step-btn', '−');
    const more = el('button', 'step-btn', '+');
    for (const b of [less, more]) b.type = 'button';
    less.setAttribute('aria-label', 'Less reasoning');
    more.setAttribute('aria-label', 'More reasoning');
    less.addEventListener('click', () => void this.step(false));
    more.addEventListener('click', () => void this.step(true));
    stepper.append(less, el('span', 'step-value', effort ?? 'unknown'), more);
    reasoning.append(stepper, el('div', 'sheet-hint', 'Applies to this session only.'));
    parts.push(reasoning);

    // Codex permissions
    if (agent === 'codex') {
      const b = row('What Codex may do…', { hint: 'Ask for approval, Approve for me, Read Only, Full Access' });
      b.addEventListener('click', () => void this.openPermissions());
      parts.push(b);
    }

    // Quick commands
    const cmds = el('div', 'more-block');
    cmds.append(el('div', 'sheet-label', 'Commands'));
    const list = el('div', 'sheet-list');
    const add = (label: string, hint: string, name: string, confirm?: string) => {
      const b = row(label, { hint });
      b.addEventListener('click', () => (confirm ? this.renderConfirm(label, confirm, () => this.command(name)) : void this.command(name)));
      list.append(b);
    };
    add('Compact', 'Summarise the conversation to free up context', 'compact');
    if (agent === 'claude') {
      add('Context', 'Show what is using the context window', 'context');
      add('Clear', 'Start a new conversation', 'clear', 'Start a new conversation? The current one stays in the agent’s history.');
    } else {
      add('New conversation', 'Start a new conversation', 'new', 'Start a new conversation? The current one stays in the agent’s history.');
    }
    cmds.append(list);
    parts.push(cmds);

    // Edit last message
    if (hasLast) {
      const b = row('Edit last message', { hint: 'Put it in the box to change and send as a new message' });
      b.addEventListener('click', () => {
        this.close();
        this.hooks.onReuse();
      });
      parts.push(b);
    }
    this.body.replaceChildren(...parts);
  }

  private renderConfirm(
    title: string,
    text: string,
    go: () => Promise<void> | void,
    danger = false,
    cancel: () => void = () => this.renderMain(),
  ) {
    const yes = el('button', danger ? 'confirm-yes danger' : 'confirm-yes', title);
    const no = el('button', 'confirm-no', 'Cancel');
    yes.type = 'button';
    no.type = 'button';
    yes.addEventListener('click', () => {
      if (danger) warn();
      void go();
    });
    no.addEventListener('click', cancel);
    const actions = el('div', 'confirm-actions');
    actions.append(no, yes);
    this.body.replaceChildren(el('div', danger ? 'confirm-text danger' : 'confirm-text', text), actions);
  }

  // ------------------------------------------------------------ reasoning

  private async step(up: boolean) {
    if (this.busy) return;
    this.busy = true;
    const { agent, effort } = this.state;
    let level: string;
    if (agent === 'codex') {
      const r = await this.call<{ level: string }>('reasoning', { up });
      this.busy = false;
      if (!r.ok) return this.renderMain(r.error);
      level = r.data.level;
    } else {
      // Claude has a slider, set by name: work out the next notch from where it is.
      const next = stepEffort(effort, up);
      if (!next) {
        this.busy = false;
        return this.renderMain(effort ? `Reasoning is already at its ${up ? 'highest' : 'lowest'}.` : 'Reasoning is not known yet.');
      }
      const r = await this.call<{ message: string }>('effort', { level: next });
      this.busy = false;
      if (!r.ok) return this.renderMain(r.error);
      level = next;
    }
    this.state = { ...this.state, effort: level };
    this.hooks.onEffort(level);
    this.renderMain();
  }

  // ------------------------------------------------------------- commands

  private async command(name: string) {
    if (this.busy) return;
    this.busy = true;
    this.message(`Sending /${name}…`);
    const r = await this.call('command', { name });
    this.busy = false;
    if (!r.ok) return this.message(r.error, true);
    this.close();
    this.hooks.onSay(`Sent /${name}`);
  }

  // ---------------------------------------------------------- permissions

  private async openPermissions() {
    if (this.busy) return;
    this.busy = true;
    this.message('Opening the permissions menu…');
    const r = await this.call<{ rows: PermRow[] }>('permissions/open');
    this.busy = false;
    if (!r.ok) return this.message(r.error, true);
    this.permsOpen = true;
    this.permRows = r.data.rows;
    this.renderPermList();
  }

  /** The presets, while Codex's own menu is still open behind them. */
  private renderPermList() {
    const list = el('div', 'sheet-list');
    for (const p of this.permRows) {
      const full = /full access/i.test(p.label);
      const b = row(p.label, {
        hint: p.description,
        mark: p.current ? '✓ now' : undefined,
        cls: full ? 'sheet-danger' : p.current ? 'current' : '',
      });
      b.addEventListener('click', () => (full ? this.confirmFull(p) : void this.choosePermission(p, false)));
      list.append(b);
    }
    const back = el('button', 'confirm-no', 'Back');
    back.type = 'button';
    back.addEventListener('click', () => void this.backFromPermissions());
    this.body.replaceChildren(el('div', 'sheet-hint', 'Applies to this session only.'), list, back);
  }

  private async backFromPermissions() {
    this.permsOpen = false;
    await this.call('model/close');
    this.renderMain();
  }

  /** Full Access is behind its own confirmation, in red, before anything is pressed. */
  private confirmFull(p: PermRow) {
    this.renderConfirm(
      'Enable Full Access',
      'Full Access lets Codex edit any file on this computer and use the network without asking you first. It applies to this session until you change it back. Enable it?',
      () => this.choosePermission(p, true),
      true,
      () => this.renderPermList(), // Codex's menu is still open: go back to its list
    );
  }

  private async choosePermission(p: PermRow, allowFull: boolean) {
    if (this.busy) return;
    this.busy = true;
    this.message(`Setting permissions to ${p.label}…`);
    const r = await this.call<{ message: string }>('permissions/choose', { number: p.number, allow_full: allowFull });
    this.busy = false;
    this.permsOpen = false; // the server closes the menu whatever happened
    if (!r.ok) return this.message(r.error, true);
    this.close();
    this.hooks.onSay(r.data.message);
  }
}
