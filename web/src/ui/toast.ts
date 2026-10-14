// A short message over the bottom of `host`, with an optional action. One per host; a new one replaces it.

import { el } from '../dom';
import { cn } from './cn';

const live = new WeakMap<HTMLElement, { node: HTMLElement; timer: number }>();

export interface ToastOptions {
  ms?: number;
  action?: { label: string; run: () => void };
}

export function toast(host: HTMLElement, message: string, opts: ToastOptions = {}): () => void {
  const prev = live.get(host);
  if (prev) {
    prev.node.remove();
    clearTimeout(prev.timer);
  }
  const node = el('div', cn('absolute inset-x-4 bottom-4 z-20 mx-auto flex max-w-md items-center gap-3 rounded-xl bg-popover px-4 py-3 text-md text-foreground shadow-pop'));
  node.setAttribute('role', 'status');
  node.append(el('span', 'min-w-0 flex-1', message));
  const dismiss = () => {
    node.remove();
    clearTimeout(entry.timer);
    if (live.get(host) === entry) live.delete(host);
  };
  if (opts.action) {
    const a = el('button', 'shrink-0 rounded-md px-2 py-1 font-semibold text-accent-text active:bg-muted', opts.action.label);
    a.type = 'button';
    a.addEventListener('click', () => {
      dismiss();
      opts.action!.run();
    });
    node.append(a);
  }
  host.append(node);
  const entry = { node, timer: window.setTimeout(dismiss, opts.ms ?? 4000) };
  live.set(host, entry);
  return dismiss;
}
