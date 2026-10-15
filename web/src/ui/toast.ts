// A short message over the bottom of `host`, with an optional action. One per host; a new one replaces it.

import { el } from '../dom';
import { button } from './button';
import { cn } from './cn';

const live = new WeakMap<HTMLElement, { node: HTMLElement; timer: number }>();

export interface ToastOptions {
  ms?: number;
  /** Extra classes, for a hook the host's styles can find. */
  cls?: string;
  /** `run` gets the button, to relabel it, and a function that closes the toast. */
  action?: ToastAction;
  /** More than one button, in order. */
  actions?: ToastAction[];
}

export interface ToastAction {
  label: string;
  run: (button: HTMLButtonElement, dismiss: () => void) => void;
}

export function toast(host: HTMLElement, message: string, opts: ToastOptions = {}): () => void {
  const prev = live.get(host);
  if (prev) {
    prev.node.remove();
    clearTimeout(prev.timer);
  }
  const node = el('div', cn('absolute inset-x-4 bottom-4 z-20 mx-auto flex max-w-md items-center gap-3 rounded-xl bg-popover px-4 py-3 text-md text-foreground shadow-pop', opts.cls));
  node.setAttribute('role', 'status');
  node.append(el('span', 'min-w-0 flex-1', message));
  const dismiss = () => {
    node.remove();
    clearTimeout(entry.timer);
    if (live.get(host) === entry) live.delete(host);
  };
  for (const act of opts.actions ?? (opts.action ? [opts.action] : [])) {
    const a = button({ variant: 'ghost', size: 'sm', cls: '-my-1 shrink-0 text-accent-text', onClick: () => act.run(a, dismiss) }, act.label);
    a.addEventListener('pointerdown', (e) => e.preventDefault()); // keep the keyboard where it was
    node.append(a);
  }
  host.append(node);
  const entry = { node, timer: window.setTimeout(dismiss, opts.ms ?? 4000) };
  live.set(host, entry);
  return dismiss;
}
