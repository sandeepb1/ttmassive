// A small menu that opens beside the button that asked for it: the `⋯` on a session row, the `+`
// on a project, the agent button in a pane's header. One at a time; any tap outside, Escape or a
// scroll closes it.

import { el } from './dom';

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
  const menu = el('div', 'menu');
  menu.setAttribute('role', 'menu');
  menu.setAttribute('aria-label', label);
  for (const it of items) {
    const b = el('button', it.danger ? 'menu-item danger' : 'menu-item');
    b.type = 'button';
    b.setAttribute('role', 'menuitem');
    b.append(el('span', 'menu-label', it.label));
    if (it.hint) b.append(el('span', 'menu-hint', it.hint));
    b.addEventListener('click', () => {
      closeMenu();
      it.run();
    });
    menu.append(b);
  }
  // Keep a tap on the menu from taking focus (and the on-screen keyboard) from the terminal.
  menu.addEventListener('pointerdown', (e) => e.preventDefault());
  document.body.append(menu);

  // Below the button, right-aligned with it; flipped above or pulled in when it would leave the screen.
  const a = anchor.getBoundingClientRect();
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const left = Math.max(8, Math.min(window.innerWidth - w - 8, a.right - w));
  const below = a.bottom + 4;
  const top = below + h > window.innerHeight - 8 ? Math.max(8, a.top - h - 4) : below;
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  anchor.setAttribute('aria-expanded', 'true');
  (menu.firstElementChild as HTMLElement | null)?.focus({ preventScroll: true });

  const onDown = (e: Event) => {
    if (e.target instanceof Node && (menu.contains(e.target) || anchor.contains(e.target))) return;
    closeMenu();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      closeMenu();
      anchor.focus({ preventScroll: true });
    }
  };
  document.addEventListener('pointerdown', onDown, true);
  document.addEventListener('keydown', onKey, true);
  window.addEventListener('scroll', closeMenu, true);
  window.addEventListener('resize', closeMenu);
  closeOpen = () => {
    closeOpen = null;
    menu.remove();
    anchor.setAttribute('aria-expanded', 'false');
    document.removeEventListener('pointerdown', onDown, true);
    document.removeEventListener('keydown', onKey, true);
    window.removeEventListener('scroll', closeMenu, true);
    window.removeEventListener('resize', closeMenu);
  };
}
