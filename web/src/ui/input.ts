// Text fields share one look; the monospace flavour is for paths and commands.

import { el } from '../dom';
import { cn } from './cn';

const field =
  'w-full min-w-0 rounded-lg bg-secondary px-4 text-md text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50';

export interface InputProps {
  type?: string;
  placeholder?: string;
  value?: string;
  mono?: boolean;
  label?: string;
  cls?: string;
}

export function input(props: InputProps = {}): HTMLInputElement {
  const i = el('input');
  i.type = props.type ?? 'text';
  i.className = cn(field, 'h-ctl', props.mono && 'font-mono', props.cls);
  if (props.placeholder) i.placeholder = props.placeholder;
  if (props.value !== undefined) i.value = props.value;
  if (props.label) i.setAttribute('aria-label', props.label);
  return i;
}

export function textarea(props: InputProps & { rows?: number } = {}): HTMLTextAreaElement {
  const t = el('textarea');
  t.className = cn(field, 'resize-none py-3', props.mono && 'font-mono', props.cls);
  t.rows = props.rows ?? 3;
  if (props.placeholder) t.placeholder = props.placeholder;
  if (props.value !== undefined) t.value = props.value;
  if (props.label) t.setAttribute('aria-label', props.label);
  return t;
}

/** A native select, so the platform picker is used on touch devices. */
export function select(options: { value: string; label: string }[], props: { value?: string; label?: string; cls?: string } = {}): HTMLSelectElement {
  const s = el('select');
  s.className = cn(field, 'h-ctl', props.cls);
  for (const o of options) s.append(new Option(o.label, o.value));
  if (props.value !== undefined) s.value = props.value;
  if (props.label) s.setAttribute('aria-label', props.label);
  return s;
}
