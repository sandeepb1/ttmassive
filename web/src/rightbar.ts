// The right sidebar: what is going on in the open session. Its model, mode and conversation
// actions, and the plan-usage numbers. (More tabs for files, to-dos and attachments come later.)

import type { ChatInfo } from './chat';
import { el } from './dom';
import { icon } from './icons';

export interface ChatActions {
  model: () => void;
  mode: () => void;
  more: () => void;
  resume: () => void;
}

export interface RightHooks {
  collapse: () => void;
  refreshUsage: () => void;
}

export class RightBar {
  private title = el('strong', 'rb-name');
  private badge = el('span', 'rb-badge');
  private status = el('div', 'rb-status');
  private rows = el('div', 'rb-rows');
  private info: ChatInfo | null = null;
  private actions: ChatActions | null = null;

  constructor(
    private root: HTMLElement,
    usageBody: HTMLElement,
    private hooks: RightHooks,
  ) {
    this.root.classList.add('sidebar', 'sidebar-right');
    this.root.setAttribute('aria-label', 'Session details');

    const head = el('div', 'sb-head');
    const hide = el('button', 'icon sb-collapse');
    hide.type = 'button';
    hide.setAttribute('aria-label', 'Hide details');
    hide.append(icon('panel-right'));
    hide.addEventListener('click', () => this.hooks.collapse());
    head.append(el('strong', 'sb-brand', 'Details'), el('span', 'sb-spacer'), hide);

    const card = el('div', 'rb-card');
    const top = el('div', 'rb-card-top');
    top.append(this.title, this.badge);
    card.append(top, this.status);

    const convo = el('section', 'rb-section');
    convo.append(el('div', 'drawer-label', 'Conversation'), this.rows);

    const usage = el('section', 'rb-section');
    const refresh = el('button', 'chip rb-refresh', 'Refresh');
    refresh.type = 'button';
    refresh.addEventListener('click', () => this.hooks.refreshUsage());
    const uhead = el('div', 'rb-uhead');
    uhead.append(el('div', 'drawer-label', 'Plan usage'), refresh);
    usage.append(uhead, usageBody);

    const body = el('div', 'rb-body');
    body.append(card, convo, usage);
    this.root.append(head, body);
    this.render();
  }

  setSession(name: string) {
    this.title.textContent = name;
    this.info = null;
    this.actions = null;
    this.render();
  }

  /** The connection or activity line, e.g. "working…". */
  setStatus(text: string) {
    this.status.textContent = text;
  }

  setInfo(info: ChatInfo | null, actions: ChatActions | null) {
    this.info = info;
    this.actions = actions;
    this.render();
  }

  private render() {
    const info = this.info;
    this.badge.hidden = !info?.agent;
    this.badge.textContent = info?.agent === 'claude' ? 'Claude' : info?.agent === 'codex' ? 'Codex' : '';
    this.badge.dataset.agent = info?.agent ?? '';

    const rows: HTMLElement[] = [];
    const a = this.actions;
    if (!info || !info.eligible || !a) {
      rows.push(el('div', 'rb-hint', 'Model, mode and conversation actions appear for Claude and Codex sessions.'));
    } else {
      rows.push(
        this.row('Model', info.model || 'Not reported yet', a.model),
        this.row('Mode', info.mode || 'Default', a.mode, info.yolo ? 'Permission checks skipped' : undefined),
        this.row('More actions', 'Reasoning, commands, edit last message', a.more),
        this.row('Resume', 'Continue an earlier conversation', a.resume),
      );
    }
    this.rows.replaceChildren(...rows);
  }

  private row(label: string, value: string, onClick: () => void, warn?: string): HTMLElement {
    const b = el('button', 'rb-row');
    b.type = 'button';
    const text = el('span', 'rb-row-text');
    text.append(el('span', 'rb-row-label', label), el('span', 'rb-row-value', value));
    if (warn) text.append(el('span', 'rb-row-warn', `⚡ ${warn}`));
    b.append(text, icon('chevron', 'icon-svg rb-chev'));
    b.addEventListener('click', onClick);
    return b;
  }
}
