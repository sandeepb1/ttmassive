// What the image viewer needs to decide, apart from the DOM so it can be tested.

/** The name a saved copy of `url` should have: its last path segment, without any query. */
export function fileName(url: string): string {
  const path = url.split(/[?#]/)[0] ?? '';
  const last = path.split('/').filter(Boolean).pop() ?? '';
  let name = last;
  try {
    name = decodeURIComponent(last);
  } catch {
    /* a stray % in the name: keep it as it is */
  }
  // A name is one path segment; nothing the URL says can put a copy anywhere else.
  name = name.replace(/[\\/:*?"<>|\u0000-\u001f]/g, '_').replace(/^\.+/, '');
  return name || 'image';
}

/** The picture `dir` steps from `index` of `count`, wrapping round at either end. */
export function step(index: number, count: number, dir: 1 | -1): number {
  if (count <= 0) return 0;
  return (((index + dir) % count) + count) % count;
}

/** `2 of 5`, or nothing for a single picture. */
export function countLabel(index: number, count: number): string {
  return count > 1 ? `${index + 1} of ${count}` : '';
}

/** How far (px) a finger has to travel sideways to turn a page. */
export const SWIPE_PX = 60;

/** A sideways swipe: far enough, and clearly more across than down. -1 is back, 1 is forward. */
export function swipeDir(dx: number, dy: number): 1 | -1 | 0 {
  if (Math.abs(dx) < SWIPE_PX || Math.abs(dx) < Math.abs(dy) * 1.5) return 0;
  return dx < 0 ? 1 : -1;
}

/** Whether a click is a plain one, which the viewer takes; with a modifier it is left to the browser. */
export function isPlainClick(e: { button: number; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean }): boolean {
  return e.button === 0 && !e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey;
}
