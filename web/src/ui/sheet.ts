// The bottom-sheet shell: a backdrop and a dialog that slide up over `host` (a pane, or the whole app).
// It is a non-modal <dialog> scoped to its host, not showModal(): sheets cover one pane of a split view
// and must leave the others usable. The shell supplies what showModal() would have: Escape, focus
// return and a drag-to-dismiss handle. It does not take focus on open, so the on-screen keyboard stays put.

import { el } from '../dom';
import { dragToDismiss } from '../gestures';
import { pushOverlay } from '../overlays';
import { icon } from '../icons';
import { button } from './button';
import { cn } from './cn';

/** Matches an open sheet. */
export const SHEET_SELECTOR = '[data-sheet]';

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
    onDismiss();
    // A sheet that closed carries on from where the finger left it; one that refused to close goes back.
    if (dialog.dataset.sheet !== undefined) dialog.style.transform = '';
  });

  let returnTo: Element | null = null;
  let releaseOverlay: (() => void) | null = null; // Back closes the sheet; see overlay-stack.ts
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
      // Not dialog.show(): its focusing steps focus the first control inside the sheet while it is still parked below
      // the screen, and the browser scrolls the app to reveal it (the whole UI jumps up, then settles). The sheet does
      // not take focus anyway (see the header), so open it by the attribute alone.
      dialog.setAttribute('open', '');
      document.addEventListener('keydown', onKey, true);
      releaseOverlay = pushOverlay(() => onDismiss());
    },
    remove() {
      document.removeEventListener('keydown', onKey, true);
      releaseOverlay?.();
      releaseOverlay = null;
      // The sheet is gone as far as the app is concerned (no longer matched by SHEET_SELECTOR, no longer tappable),
      // but stays on screen for the length of its exit animation.
      delete dialog.dataset.sheet;
      dialog.inert = true;
      backdrop.style.pointerEvents = 'none';
      dialog.classList.remove('animate-[kit-sheet-in_.28s_var(--ease)]');
      dialog.classList.add('animate-[kit-sheet-out_.2s_var(--ease)_forwards]');
      backdrop.classList.remove('animate-[kit-fade-in_.2s_ease]');
      backdrop.classList.add('animate-[kit-fade-out_.2s_ease_forwards]');
      let done = false;
      const finish = () => {
        if (done) return;
        done = true;
        dialog.removeAttribute('open');
        backdrop.remove();
        dialog.remove();
      };
      dialog.addEventListener('animationend', finish, { once: true });
      window.setTimeout(finish, 300);
      if (returnTo instanceof HTMLElement && returnTo.isConnected) returnTo.focus({ preventScroll: true });
      returnTo = null;
    },
  };
}
