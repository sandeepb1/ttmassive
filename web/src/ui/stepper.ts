// A minus button, a value and a plus button: reasoning level, text size, pane count.

import { el } from '../dom';
import { icon } from '../icons';
import { button } from './button';

export interface Stepper {
  el: HTMLElement;
  /** Where the current value is shown. */
  out: HTMLElement;
  minus: HTMLButtonElement;
  plus: HTMLButtonElement;
}

export function stepper(labels: { less: string; more: string }, onStep: (dir: -1 | 1) => void): Stepper {
  const out = el('span', 'min-w-24 text-center text-lg font-bold tabular-nums');
  const make = (label: string, glyph: 'minus' | 'plus', dir: -1 | 1) =>
    button({ variant: 'secondary', size: 'icon', label, cls: 'size-13 rounded-full', onClick: () => onStep(dir) }, icon(glyph, 'size-5'));
  const minus = make(labels.less, 'minus', -1);
  const plus = make(labels.more, 'plus', 1);
  const wrap = el('div', 'flex items-center justify-center gap-5 py-2');
  wrap.append(minus, out, plus);
  return { el: wrap, out, minus, plus };
}
