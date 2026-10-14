// The bottom-sheet shell: a backdrop and a dialog that slide up over `host` (a pane, or the whole app).
// It is a non-modal <dialog> scoped to its host, not showModal(): sheets cover one pane of a split view
// and must leave the others usable. The shell supplies what showModal() would have: Escape, focus
// return and a drag-to-dismiss handle. It does not take focus on open, so the on-screen keyboard stays put.

import { el } from '../dom';
import { dragToDismiss } from '../gestures';
import { icon } from '../icons';
import { button } from './button';
import { cn } from './cn';

/** Matches an open sheet, kit or legacy (the model picker still uses the old markup). */
export const SHEET_SELECTOR = '[data-sheet], .sheet';

export interface SheetShell {
  /** Put the sheet on screen. */
  open(host: HTMLElement): void;
  /** Take it off screen without calling `onDismiss`. */
  remove(): void;
  /** Where the content goes. */
  body: HTMLElement;
  tall(): void;
  wide(): void;
}

/** `onDismiss` runs when the user closes the sheet: backdrop, ×, Escape or a drag. */
export function sheetShell(title: string, onDismiss: () => void): SheetShell {
  const backdrop = el('div', 'absolute inset-0 z-10 animate-[kit-fade-in_.2s_ease] bg-(--scrim)');
  const dialog = el(
    'dialog',
    cn(
      'absolute inset-x-auto bottom-0 left-1/2 top-auto z-11 m-0 flex max-h-[82%] w-[min(100%,35rem)] max-w-none -translate-x-1/2 flex-col',
      'animate-[kit-sheet-in_.28s_var(--ease)] rounded-t-xl border-0 border-t border-border bg-card px-4 pb-[calc(1rem+env(safe-area-inset-bottom))] pt-2.5 text-foreground',
      'transition-transform duration-200 ease-(--ease) [&.dragging]:transition-none',
    ),
  );
  dialog.dataset.sheet = '';
  dialog.setAttribute('aria-label', title);
  const body = el('div', 'flex flex-col gap-3.5 overflow-y-auto');

  // A tap on a button must not take focus from the message box: that would drop the on-screen
  // keyboard and make the sheet jump under the next tap.
  dialog.addEventListener('pointerdown', (e) => {
    if (e.target instanceof Element && e.target.closest('button')) e.preventDefault();
  });

  const head = el('div', 'relative flex touch-none cursor-grab items-center justify-between pb-1.5 pt-4 text-lg');
  head.append(
    el('span', 'absolute left-1/2 top-1.5 h-1 w-9 -translate-x-1/2 rounded-full bg-input'),
    el('strong', undefined, title),
    button({ variant: 'ghost', size: 'icon-sm', label: 'Close', cls: '-mr-2', onClick: () => onDismiss() }, icon('close')),
  );
  dialog.append(head, body);
  dragToDismiss(dialog, head, () => {
    dialog.style.transform = '';
    onDismiss();
  });

  let returnTo: Element | null = null;
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== 'Escape' || e.defaultPrevented) return;
    e.stopPropagation();
    onDismiss();
  };
  backdrop.addEventListener('click', () => onDismiss());

  return {
    body,
    tall() {
      dialog.classList.add('h-[92%]', 'max-h-[92%]');
      body.classList.add('min-h-0', 'flex-1', 'gap-0', 'overflow-hidden', '[&_.agentview]:-mx-4');
    },
    wide() {
      dialog.classList.replace('w-[min(100%,35rem)]', 'w-[min(100%,68.75rem)]');
    },
    open(host) {
      returnTo = document.activeElement;
      host.append(backdrop, dialog);
      dialog.show();
      document.addEventListener('keydown', onKey, true);
    },
    remove() {
      document.removeEventListener('keydown', onKey, true);
      dialog.close();
      backdrop.remove();
      dialog.remove();
      if (returnTo instanceof HTMLElement && returnTo.isConnected) returnTo.focus({ preventScroll: true });
      returnTo = null;
    },
  };
}
