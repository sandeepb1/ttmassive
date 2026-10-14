// A full-screen viewer for pictures in the chat and the sidebar: the picture as large as it fits,
// with Save and Open in new tab, and arrows (or a swipe) to move between the pictures that were
// shown together. Escape, the close button or a tap outside the picture leaves it.

import { el } from './dom';
import { countLabel, fileName, isPlainClick, step, swipeDir } from './image-viewer-model';

let current: { close: () => void } | null = null;

/** Show `urls[start]` in the viewer. A second call replaces the first. */
export function openImageViewer(urls: string[], start = 0): void {
  if (urls.length === 0) return;
  current?.close();
  let index = Math.min(Math.max(start, 0), urls.length - 1);
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  const root = el('div', 'img-viewer');
  root.setAttribute('role', 'dialog');
  root.setAttribute('aria-modal', 'true');
  root.setAttribute('aria-label', 'Image');

  const count = el('span', 'iv-count');
  const save = el('a', 'iv-btn', 'Save');
  const open = el('a', 'iv-btn', 'Open in new tab');
  open.target = '_blank';
  open.rel = 'noopener';
  const close = el('button', 'iv-btn iv-close', '×');
  close.type = 'button';
  close.setAttribute('aria-label', 'Close');
  const bar = el('div', 'iv-bar');
  bar.append(count, el('span', 'iv-spacer'), save, open, close);

  const img = el('img', 'iv-img');
  const note = el('div', 'iv-note');
  note.hidden = true;
  const stage = el('div', 'iv-stage');
  const prev = el('button', 'iv-nav iv-prev', '‹');
  const next = el('button', 'iv-nav iv-next', '›');
  for (const [b, label] of [[prev, 'Previous image'], [next, 'Next image']] as const) {
    b.type = 'button';
    b.setAttribute('aria-label', label);
    b.hidden = urls.length < 2;
  }
  stage.append(prev, img, note, next);
  root.append(bar, stage);

  const show = () => {
    const url = urls[index]!;
    note.hidden = true;
    img.hidden = false;
    img.alt = `Image ${index + 1} of ${urls.length}`;
    img.src = url;
    save.href = url;
    save.download = fileName(url);
    open.href = url;
    count.textContent = countLabel(index, urls.length);
  };
  const go = (dir: 1 | -1) => {
    if (urls.length < 2) return;
    index = step(index, urls.length, dir);
    show();
  };

  img.addEventListener('error', () => {
    img.hidden = true;
    note.textContent = 'This image could not be loaded. It may have been removed.';
    note.hidden = false;
  });
  close.addEventListener('click', () => dismiss());
  prev.addEventListener('click', () => go(-1));
  next.addEventListener('click', () => go(1));
  // A tap on the dark around the picture leaves; a tap on the picture or its buttons does not.
  stage.addEventListener('click', (e) => {
    if (e.target === stage) dismiss();
  });

  let from: { x: number; y: number } | null = null;
  stage.addEventListener('pointerdown', (e) => {
    if (e.pointerType === 'touch' && e.isPrimary) from = { x: e.clientX, y: e.clientY };
  });
  stage.addEventListener('pointerup', (e) => {
    if (!from) return;
    const dir = swipeDir(e.clientX - from.x, e.clientY - from.y);
    from = null;
    if (dir !== 0) go(dir);
  });
  stage.addEventListener('pointercancel', () => (from = null));

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      dismiss();
    } else if (e.key === 'ArrowLeft') go(-1);
    else if (e.key === 'ArrowRight') go(1);
    else if (e.key === 'Tab') {
      // Keep the focus inside: the page behind is not reachable while this is up.
      const focusable = [...root.querySelectorAll<HTMLElement>('a[href], button')].filter((x) => !x.hidden && x.getClientRects().length > 0);
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) return;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  let closed = false;
  function dismiss() {
    if (closed) return;
    closed = true;
    document.removeEventListener('keydown', onKey, true);
    root.remove();
    if (current === handle) current = null;
    // Back to what had the focus, unless it has gone.
    if (opener?.isConnected) opener.focus({ preventScroll: true });
  }
  const handle = { close: dismiss };
  current = handle;

  show();
  document.addEventListener('keydown', onKey, true);
  document.body.append(root);
  close.focus({ preventScroll: true });
}

/**
 * Make a picture's link open the viewer on a plain click. With a modifier key, or a middle click,
 * the browser keeps it: the link still points at the picture.
 */
export function viewable(link: HTMLAnchorElement, urls: () => string[], index: number): void {
  link.addEventListener('click', (e) => {
    if (!isPlainClick(e)) return;
    e.preventDefault();
    openImageViewer(urls(), index);
  });
}
