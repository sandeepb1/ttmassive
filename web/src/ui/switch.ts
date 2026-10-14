// An on/off switch. State lives in aria-checked, which the styles read.

import { el } from '../dom';
import { cn } from './cn';

export interface SwitchProps {
  checked: boolean;
  label: string;
  onChange?: (checked: boolean) => void;
  cls?: string;
}

export function toggleSwitch(props: SwitchProps): HTMLButtonElement {
  const b = el('button');
  b.type = 'button';
  b.setAttribute('role', 'switch');
  b.setAttribute('aria-label', props.label);
  b.setAttribute('aria-checked', String(props.checked));
  b.className = cn(
    'relative h-8 w-14 shrink-0 touch-manipulation touch:before:absolute touch:before:-inset-y-2.5 touch:before:inset-x-0 rounded-full bg-secondary transition-colors aria-checked:bg-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
    props.cls,
  );
  b.append(el('span', 'absolute left-1 top-1 size-6 rounded-full bg-foreground transition-transform in-aria-checked:translate-x-6 in-aria-checked:bg-primary-foreground'));
  b.addEventListener('click', () => {
    const next = b.getAttribute('aria-checked') !== 'true';
    b.setAttribute('aria-checked', String(next));
    props.onChange?.(next);
  });
  return b;
}
