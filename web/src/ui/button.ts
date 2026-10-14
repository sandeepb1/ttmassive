// The one button. Variants set colour, sizes set the control height (direction D: 48px touch targets).

import { el } from '../dom';
import { cn, tv } from './cn';

export const buttonStyles = tv({
  base: 'inline-flex shrink-0 touch-manipulation select-none items-center justify-center gap-2 rounded-lg font-semibold transition active:scale-[.98] disabled:pointer-events-none disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
  variants: {
    variant: {
      default: 'bg-primary text-primary-foreground',
      secondary: 'bg-secondary text-foreground active:bg-muted',
      outline: 'border border-border bg-transparent text-foreground active:bg-muted',
      ghost: 'bg-transparent text-foreground active:bg-muted',
      destructive: 'bg-destructive text-destructive-foreground',
      'soft-danger': 'bg-danger-bg text-danger-text',
    },
    size: {
      md: 'h-ctl px-5 text-md',
      sm: 'h-ctl-sm px-4 text-sm',
      icon: 'size-ctl',
      'icon-sm': 'size-ctl-sm',
    },
  },
  defaultVariants: { variant: 'default', size: 'md' },
});

export interface ButtonProps {
  variant?: 'default' | 'secondary' | 'outline' | 'ghost' | 'destructive' | 'soft-danger';
  size?: 'md' | 'sm' | 'icon' | 'icon-sm';
  /** Accessible name; required for icon-only buttons. */
  label?: string;
  cls?: string;
  disabled?: boolean;
  onClick?: (e: MouseEvent) => void;
}

/** A button with text and/or icons as children. */
export function button(props: ButtonProps, ...children: (Node | string)[]): HTMLButtonElement {
  const b = el('button');
  b.type = 'button';
  b.className = cn(buttonStyles({ variant: props.variant, size: props.size }), props.cls);
  if (props.label) b.setAttribute('aria-label', props.label);
  if (props.disabled) b.disabled = true;
  if (props.onClick) b.addEventListener('click', props.onClick);
  b.append(...children);
  return b;
}
