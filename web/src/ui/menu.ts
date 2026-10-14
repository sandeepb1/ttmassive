// A small menu that opens beside the button that asked for it: the `⋯` on a session row, the `+`
// on a project, the agent button in a pane's header. One at a time; any tap outside, Escape or a
// scroll closes it. It lives in the top layer (popover), so it also shows over an open sheet.

import { autoUpdate, computePosition, flip, offset, shift, size } from '@floating-ui/dom';
import { el } from '../dom';
import { cn } from './cn';

export interface MenuItem {
  label: string;
  /** A short second line, or the mark on the right. */
  hint?: string;
  danger?: boolean;
  run: () => void;
}

let closeOpen: (() => void) | null = null;

export function closeMenu() {
  closeOpen?.();
}

/** Show `items` next to `anchor`. Returns nothing: the menu closes itself. */
export function openMenu(anchor: HTMLElement, items: MenuItem[], label = 'Actions') {
  closeMenu();
  if (items.length === 0) return;
  const menu = el(
    'div',
    'fixed inset-auto m-0 flex min-w-52 max-w-[min(20rem,calc(100vw-1rem))] flex-col gap-0.5 overflow-y-auto rounded-xl border-0 bg-popover p-1.5 text-foreground shadow-pop',
  );
  menu.popover = 'manual';
  menu.setAttribute('role', 'menu');
  menu.setAttribute('aria-label', label);
  const buttons: HTMLButtonElement[] = [];
  for (const it of items) {
    const b = el(
      'button',
      cn(
        'flex min-h-ctl w-full flex-col items-start justify-center rounded-lg px-3 py-2 text-left transition-colors hover:bg-muted focus-visible:bg-muted focus-visible:outline-0 active:bg-secondary',
        it.danger && 'text-danger-text',
      ),
    );
    b.type = 'button';
    b.setAttribute('role', 'menuitem');
    b.append(el('span', 'text-md font-semibold', it.label));
    if (it.hint) b.append(el('span', 'text-2xs text-muted-foreground', it.hint));
    b.addEventListener('click', () => {
      closeMenu();
      it.run();
    });
    buttons.push(b);
    menu.append(b);
  }
  // Keep a tap on the menu from taking focus (and the on-screen keyboard) from the terminal.
  menu.addEventListener('pointerdown', (e) => e.preventDefault());
  document.body.append(menu);
  menu.showPopover();

  // Below the button, right-aligned with it; flipped above or pulled in when it would leave the screen.
  const place = () =>
    computePosition(anchor, menu, {
      strategy: 'fixed',
      placement: 'bottom-end',
      middleware: [
        offset(6),
        flip({ padding: 8 }),
        shift({ padding: 8 }),
        size({ padding: 8, apply: ({ availableHeight }) => {
          menu.style.maxHeight = `${Math.max(120, availableHeight)}px`;
        },
      }),
      ],
    }).then(({ x, y }) => {
      menu.style.left = `${x}px`;
      menu.style.top = `${y}px`;
    });
  const stopUpdate = autoUpdate(anchor, menu, place, { animationFrame: false });
  anchor.setAttribute('aria-expanded', 'true');
  buttons[0]?.focus({ preventScroll: true });

  const onDown = (e: Event) => {
    if (e.target instanceof Node && (menu.contains(e.target) || anchor.contains(e.target))) return;
    closeMenu();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      closeMenu();
      anchor.focus({ preventScroll: true });
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = (at + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
      buttons[next]?.focus({ preventScroll: true });
    }
  };
  // autoUpdate follows the anchor on scroll, but the old behaviour closes the menu when the page scrolls.
  const onScroll = (e: Event) => {
    if (!(e.target instanceof Node) || !menu.contains(e.target)) closeMenu();
  };
  document.addEventListener('pointerdown', onDown, true);
  document.addEventListener('keydown', onKey, true);
  window.addEventListener('scroll', onScroll, true);
  window.addEventListener('resize', closeMenu);
  closeOpen = () => {
    closeOpen = null;
    stopUpdate();
    menu.hidePopover();
    menu.remove();
    anchor.setAttribute('aria-expanded', 'false');
    document.removeEventListener('pointerdown', onDown, true);
    document.removeEventListener('keydown', onKey, true);
    window.removeEventListener('scroll', onScroll, true);
    window.removeEventListener('resize', closeMenu);
  };
}
