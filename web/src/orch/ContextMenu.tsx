// A small right-click / long-press menu. The pages decide what is in it; this places it, keeps it on screen, and
// closes it on Escape, a click elsewhere, scrolling or resizing. Arrow keys move through the items.

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Icon } from './bits';
import type { IconName } from '../icons';

export type MenuItem =
  | { kind?: 'item'; label: string; icon?: IconName; danger?: boolean; disabled?: boolean; onSelect: () => void }
  | { kind: 'separator' };

export interface MenuState {
  x: number;
  y: number;
  items: MenuItem[];
}

const LONG_PRESS_MS = 520;

/** Right-click handler props for an element. */
export function menuOn(open: (m: MenuState) => void, items: () => MenuItem[]) {
  return {
    onContextMenu: (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      open({ x: e.clientX, y: e.clientY, items: items() });
    },
  };
}

/**
 * Touch long-press for elements where the browser sends no context menu (iOS). Android also sends one, so a press
 * that already opened a menu is not opened twice.
 */
export function useLongPress(open: (m: MenuState) => void) {
  const timer = useRef<number>(0);
  const fired = useRef(false);
  const cancel = () => window.clearTimeout(timer.current);
  return (items: () => MenuItem[]) => ({
    onPointerDown: (e: React.PointerEvent) => {
      if (e.pointerType !== 'touch') return;
      const { clientX: x, clientY: y } = e;
      fired.current = false;
      cancel();
      timer.current = window.setTimeout(() => {
        fired.current = true;
        open({ x, y, items: items() });
      }, LONG_PRESS_MS);
    },
    onPointerMove: (e: React.PointerEvent) => {
      if (e.pointerType === 'touch' && Math.abs(e.movementX) + Math.abs(e.movementY) > 6) cancel();
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
    // The tap that ends a long press must not also activate the row.
    onClickCapture: (e: React.MouseEvent) => {
      if (fired.current) {
        fired.current = false;
        e.preventDefault();
        e.stopPropagation();
      }
    },
  });
}

export function ContextMenu({ menu, onClose }: { menu: MenuState; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [at, setAt] = useState({ left: menu.x, top: menu.y });

  // Keep it inside the window: flip up or left when it would run off the edge.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const pad = 8;
    setAt({
      left: Math.max(pad, Math.min(menu.x, window.innerWidth - width - pad)),
      top: Math.max(pad, Math.min(menu.y, window.innerHeight - height - pad)),
    });
    el.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus({ preventScroll: true });
  }, [menu]);

  useEffect(() => {
    const away = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') return onClose();
      if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
      const items = [...(ref.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
      if (!items.length) return;
      e.preventDefault();
      const i = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = e.key === 'Home' ? 0 : e.key === 'End' ? items.length - 1 : (i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    };
    document.addEventListener('pointerdown', away, true);
    document.addEventListener('contextmenu', away, true);
    document.addEventListener('keydown', key);
    window.addEventListener('resize', onClose);
    window.addEventListener('blur', onClose);
    document.addEventListener('scroll', onClose, true);
    return () => {
      document.removeEventListener('pointerdown', away, true);
      document.removeEventListener('contextmenu', away, true);
      document.removeEventListener('keydown', key);
      window.removeEventListener('resize', onClose);
      window.removeEventListener('blur', onClose);
      document.removeEventListener('scroll', onClose, true);
    };
  }, [onClose]);

  return createPortal(
    <div ref={ref} className="mc-menu" role="menu" style={at} onContextMenu={(e) => e.preventDefault()}>
      {menu.items.map((it, i) =>
        it.kind === 'separator' ? (
          <div key={i} className="mc-menu-sep" role="separator" />
        ) : (
          <button
            key={i}
            type="button"
            role="menuitem"
            className="mc-menu-item"
            data-danger={it.danger || undefined}
            disabled={it.disabled}
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            <span className="mc-menu-ico">{it.icon && <Icon name={it.icon} />}</span>
            {it.label}
          </button>
        ),
      )}
    </div>,
    document.getElementById('orch') ?? document.body,
  );
}
