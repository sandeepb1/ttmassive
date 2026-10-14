// A tappable list row: a title, an optional mark on the right and a description under it.

import { el } from '../dom';
import { cn, tv } from './cn';

const rowStyles = tv({
  base: 'flex w-full touch-manipulation flex-col gap-0.5 rounded-lg bg-card px-4 py-3 text-left text-foreground transition active:scale-[.99] active:bg-muted focus-visible:outline-2 focus-visible:outline-ring',
  variants: {
    current: { true: 'ring-2 ring-primary', false: '' },
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
  b.className = cn(rowStyles({ current: !!opts.current, danger: !!opts.danger }), opts.cls);
  const top = el('span', 'flex items-center justify-between gap-3');
  top.append(el('span', cn('text-md font-semibold', opts.danger && 'text-danger-text'), label));
  if (opts.mark) top.append(el('span', 'text-xs text-muted-foreground', opts.mark));
  b.append(top);
  if (opts.hint) b.append(el('span', 'text-sm text-muted-foreground', opts.hint));
  return b;
}
