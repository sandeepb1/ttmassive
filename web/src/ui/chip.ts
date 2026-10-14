// Pills: `badge` is a static label, `chip` is a toggleable button (a filter, a quick reply).

import { el } from '../dom';
import { cn, tv } from './cn';

export type Tone = 'neutral' | 'accent' | 'ok' | 'warn' | 'danger';

const badgeStyles = tv({
  base: 'inline-flex h-ctl-xs shrink-0 items-center gap-1 whitespace-nowrap rounded-full px-3 text-xs font-medium',
  variants: {
    tone: {
      neutral: 'bg-muted text-muted-foreground',
      accent: 'bg-accent-bg text-accent-text',
      ok: 'bg-ok-bg text-ok-text',
      warn: 'bg-warn-bg text-warn-text',
      danger: 'bg-danger-bg text-danger-text',
    },
  },
  defaultVariants: { tone: 'neutral' },
});

export function badge(props: { tone?: Tone; cls?: string }, ...children: (Node | string)[]): HTMLSpanElement {
  const s = el('span');
  s.className = cn(badgeStyles({ tone: props.tone }), props.cls);
  s.append(...children);
  return s;
}

const chipStyles = tv({
  base: 'inline-flex h-ctl-sm shrink-0 touch-manipulation items-center gap-2 rounded-full px-4 text-sm font-semibold transition active:scale-[.98] disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
  variants: {
    tone: {
      neutral: 'bg-secondary text-foreground active:bg-muted',
      accent: 'bg-accent-bg text-accent-text',
      ok: 'bg-ok-bg text-ok-text',
      warn: 'bg-warn-bg text-warn-text',
      danger: 'bg-danger-bg text-danger-text',
    },
  },
  defaultVariants: { tone: 'neutral' },
});

export interface ChipProps {
  /** Shorthand for the accent tone, and sets aria-pressed. */
  on?: boolean;
  tone?: Tone;
  cls?: string;
  onClick?: (e: MouseEvent) => void;
}

export function chip(props: ChipProps, ...children: (Node | string)[]): HTMLButtonElement {
  const b = el('button');
  b.type = 'button';
  b.dataset.cls = props.cls ?? '';
  b.className = cn(chipStyles({ tone: props.on ? 'accent' : props.tone }), props.cls);
  if (props.on !== undefined) b.setAttribute('aria-pressed', String(props.on));
  if (props.onClick) b.addEventListener('click', props.onClick);
  b.append(...children);
  return b;
}

/** Change a chip's tone in place, keeping the extra classes it was made with. */
export function setChipTone(b: HTMLElement, tone: Tone) {
  b.className = cn(chipStyles({ tone }), b.dataset.cls);
}

/** Flip a toggle chip's look and aria-pressed in place. */
export function setChipOn(b: HTMLElement, on: boolean) {
  b.setAttribute('aria-pressed', String(on));
  setChipTone(b, on ? 'accent' : 'neutral');
}
