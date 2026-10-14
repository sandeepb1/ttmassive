// A tappable list row: a title, an optional mark on the right and a description under it.

import { el } from '../dom';
import { cn, tv } from './cn';

const rowStyles = tv({
  base: 'flex w-full touch-manipulation flex-col gap-0.5 rounded-lg bg-muted px-4 py-3 text-left text-foreground transition active:scale-[.99] active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring',
  variants: {
    current: { true: 'bg-accent-bg ring-2 ring-inset ring-primary', false: '' },
    danger: { true: 'bg-danger-bg', false: '' },
  },
});

export interface RowOptions {
  hint?: string;
  mark?: string;
  current?: boolean;
  danger?: boolean;
  cls?: string;
}

export function row(label: string, opts: RowOptions = {}): HTMLButtonElement {
  const b = el('button');
  b.type = 'button';
  b.dataset.cls = opts.cls ?? '';
  if (opts.danger) b.dataset.danger = '1';
  b.className = cn(rowStyles({ current: !!opts.current, danger: !!opts.danger }), opts.cls);
  const top = el('span', 'flex items-center justify-between gap-3');
  top.append(el('span', cn('text-md font-semibold', opts.danger && 'text-danger-text'), label));
  if (opts.mark) top.append(el('span', 'text-xs text-accent-text', opts.mark));
  b.append(top);
  if (opts.hint) b.append(el('span', 'text-sm text-muted-foreground', opts.hint));
  return b;
}

/** Mark a row as the chosen one, or not, in place. */
export function setRowCurrent(b: HTMLElement, current: boolean) {
  b.classList.toggle('is-current', current);
  b.setAttribute('aria-pressed', String(current));
  b.className = cn(rowStyles({ current, danger: b.dataset.danger === '1' }), b.dataset.cls);
}
