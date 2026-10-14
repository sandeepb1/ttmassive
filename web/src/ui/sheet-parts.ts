// The pieces sheets are made of: a section label, a hint, a list of rows, a message.

import { el } from '../dom';
import { cn } from './cn';

/** A small uppercase heading over a group. */
export function sheetLabel(text: string): HTMLElement {
  return el('div', 'mb-1.5 text-xs uppercase tracking-wider text-muted-foreground', text);
}

export function sheetHint(text: string, cls?: string): HTMLElement {
  return el('div', cn('text-xs text-muted-foreground', cls), text);
}

/** Rows stacked with a small gap. */
export function sheetList(...children: Node[]): HTMLElement {
  const l = el('div', 'flex flex-col gap-1.5');
  l.append(...children);
  return l;
}

/** A label and what belongs under it. */
export function sheetBlock(...children: Node[]): HTMLElement {
  const b = el('div', 'flex flex-col gap-1.5');
  b.append(...children);
  return b;
}

/** One line in the middle of an otherwise empty sheet: loading, or what went wrong. */
export function sheetMsg(text: string, isError = false): HTMLElement {
  return el('div', cn('px-2 py-7 text-center', isError ? 'text-danger-text' : 'text-muted-foreground'), text);
}

/** Buttons along the bottom of a confirmation, the main one last. */
export function sheetActions(...children: Node[]): HTMLElement {
  const a = el('div', 'flex justify-end gap-2.5');
  a.append(...children);
  return a;
}
