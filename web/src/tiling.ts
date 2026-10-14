// Tiled panes: which preset grids exist, which fit the screen, which panes are on show, and how
// the dividers move. Everything here is pure so node can test it; `main.ts` applies the result.

export type PresetId = 'single' | 'cols2' | 'rows2' | 'cols3' | 'main-side' | 'grid2x2' | 'grid3x2';

export interface Preset {
  id: PresetId;
  label: string;
  /** Tile names laid out as a grid (CSS grid-template-areas). A name used twice is one bigger tile. */
  areas: string[][];
  /** Default column and row sizes, as relative weights. */
  cols: number[];
  rows: number[];
}

export const PRESETS: Preset[] = [
  { id: 'single', label: 'One pane', areas: [['a']], cols: [1], rows: [1] },
  { id: 'cols2', label: 'Two columns', areas: [['a', 'b']], cols: [1, 1], rows: [1] },
  { id: 'rows2', label: 'Two rows', areas: [['a'], ['b']], cols: [1], rows: [1, 1] },
  { id: 'cols3', label: 'Three columns', areas: [['a', 'b', 'c']], cols: [1, 1, 1], rows: [1] },
  { id: 'main-side', label: 'One big, two small', areas: [['a', 'b'], ['a', 'c']], cols: [2, 1], rows: [1, 1] },
  { id: 'grid2x2', label: 'Grid 2×2', areas: [['a', 'b'], ['c', 'd']], cols: [1, 1], rows: [1, 1] },
  { id: 'grid3x2', label: 'Grid 3×2', areas: [['a', 'b', 'c'], ['d', 'e', 'f']], cols: [1, 1, 1], rows: [1, 1] },
];

export const DEFAULT_PRESET: PresetId = 'cols2';

/** A tile smaller than this stops being a usable terminal (about 45 columns, 12 rows). */
export const MIN_TILE_W = 380;
export const MIN_TILE_H = 260;
/** Below this width (a phone) several tiles are too cramped, even stacked: panes go behind tabs instead. */
export const MIN_MULTI_W = 560;

export function preset(id: string): Preset {
  return PRESETS.find((p) => p.id === id) ?? PRESETS[0]!;
}

export const isPresetId = (s: unknown): s is PresetId => typeof s === 'string' && PRESETS.some((p) => p.id === s);

/** Tile names in the order panes fill them. */
export function tileNames(p: Preset): string[] {
  const out: string[] = [];
  for (const row of p.areas) for (const n of row) if (!out.includes(n)) out.push(n);
  return out;
}

export const tileCount = (p: Preset) => tileNames(p).length;

/** Does every tile of `p` get at least the minimum size in a `w` by `h` area? */
export function fits(p: Preset, w: number, h: number): boolean {
  if (tileCount(p) > 1 && w < MIN_MULTI_W) return false;
  return p.cols.length * MIN_TILE_W <= w && p.rows.length * MIN_TILE_H <= h;
}

/** Presets that fit and stay within `max` tiles. `single` always qualifies, so this is never empty. */
export function usable(w: number, h: number, max: number): Preset[] {
  return PRESETS.filter((p) => p.id === 'single' || (tileCount(p) <= max && fits(p, w, h)));
}

/**
 * The preset that is actually drawn: the one asked for if it fits, otherwise the biggest smaller
 * one that does (a phone asking for 2×2 gets a single pane and tabs).
 */
export function effectivePreset(asked: PresetId, w: number, h: number, max: number): Preset {
  const want = preset(asked);
  const ok = usable(w, h, max);
  if (ok.some((p) => p.id === want.id)) return want;
  const smaller = ok.filter((p) => tileCount(p) <= tileCount(want));
  return smaller.reduce((best, p) => (tileCount(p) > tileCount(best) ? p : best), PRESETS[0]!);
}

/**
 * Opening another pane when every tile is taken: move up to the smallest preset with room, if one
 * fits. Returns `asked` unchanged when there is room already or nothing bigger fits (the extra
 * pane then lives behind a tab).
 */
export function presetForCount(asked: PresetId, count: number, w: number, h: number, max: number): PresetId {
  if (tileCount(effectivePreset(asked, w, h, max)) >= count) return asked;
  const bigger = usable(w, h, max)
    .filter((p) => tileCount(p) >= count)
    .sort((a, b) => tileCount(a) - tileCount(b));
  return bigger[0]?.id ?? asked;
}

/**
 * Which panes are on screen, in tile order. The focused pane is always on screen: when it is
 * beyond the tile count it takes the last tile.
 */
export function visiblePanes(count: number, focused: number, capacity: number): number[] {
  const n = Math.min(count, capacity);
  const idx = Array.from({ length: n }, (_, i) => i);
  if (n > 0 && focused >= n && focused < count) idx[n - 1] = focused;
  return idx;
}

/** Corner tiles, where the sidebar buttons live: list button top-left, details button top-right. */
export function corners(p: Preset): { first: string; last: string } {
  const top = p.areas[0]!;
  return { first: top[0]!, last: top[top.length - 1]! };
}

// ---------------------------------------------------------------- sizes and dividers

/** Turn weights into fractions that add to 1; anything unusable falls back to equal sizes. */
export function normalize(weights: number[]): number[] {
  const total = weights.reduce((a, b) => a + b, 0);
  if (!weights.length) return [];
  if (!(total > 0) || weights.some((w) => !(w > 0) || !Number.isFinite(w))) return weights.map(() => 1 / weights.length);
  return weights.map((w) => w / total);
}

/** Saved sizes for `p`, or its defaults when the saved ones do not match its shape. */
export function sanitizeSizes(p: Preset, cols: unknown, rows: unknown): { cols: number[]; rows: number[] } {
  const pick = (raw: unknown, def: number[]) =>
    Array.isArray(raw) && raw.length === def.length && raw.every((x) => typeof x === 'number') ? normalize(raw as number[]) : normalize(def);
  return { cols: pick(cols, p.cols), rows: pick(rows, p.rows) };
}

/** Move the divider after track `i` by `delta` (a fraction of the whole), keeping both sides above `min`. */
export function dragSizes(sizes: number[], i: number, delta: number, min = 0.12): number[] {
  if (i < 0 || i >= sizes.length - 1) return sizes;
  const a = sizes[i]!;
  const b = sizes[i + 1]!;
  const moved = Math.min(b - min, Math.max(min - a, delta));
  if (!(a + b > 2 * min)) return sizes;
  const out = sizes.slice();
  out[i] = a + moved;
  out[i + 1] = b - moved;
  return out;
}

export interface Divider {
  /** `col` is a vertical line between columns, `row` a horizontal line between rows. */
  axis: 'col' | 'row';
  /** The line sits after this column (or row). */
  after: number;
  /** The stretch it covers along the other axis: tracks `from` to `to`, inclusive. */
  from: number;
  to: number;
}

/** The draggable lines for `p`: only where two different tiles meet, so a big tile has no line through it. */
export function dividers(p: Preset): Divider[] {
  const out: Divider[] = [];
  const rows = p.areas.length;
  const cols = p.cols.length;
  for (let c = 0; c < cols - 1; c++) {
    let start = -1;
    for (let r = 0; r <= rows; r++) {
      const apart = r < rows && p.areas[r]![c] !== p.areas[r]![c + 1];
      if (apart && start < 0) start = r;
      if (!apart && start >= 0) {
        out.push({ axis: 'col', after: c, from: start, to: r - 1 });
        start = -1;
      }
    }
  }
  for (let r = 0; r < rows - 1; r++) {
    let start = -1;
    for (let c = 0; c <= cols; c++) {
      const apart = c < cols && p.areas[r]![c] !== p.areas[r + 1]![c];
      if (apart && start < 0) start = c;
      if (!apart && start >= 0) {
        out.push({ axis: 'row', after: r, from: start, to: c - 1 });
        start = -1;
      }
    }
  }
  return out;
}

/** Where track `i` starts and ends, as fractions of the whole. */
export function span(sizes: number[], from: number, to: number): [number, number] {
  let start = 0;
  for (let i = 0; i < from; i++) start += sizes[i]!;
  let end = start;
  for (let i = from; i <= to; i++) end += sizes[i]!;
  return [start, end];
}

// ---------------------------------------------------------------- keyboard and defaults

export type Dir = 'left' | 'right' | 'up' | 'down';

/**
 * The tile you land on by moving from tile `slot` in `dir`, or null at the edge. Moves from the
 * tile's top-left cell, so a big tile always steps to the same neighbour.
 */
export function neighbor(p: Preset, slot: number, dir: Dir): number | null {
  const names = tileNames(p);
  const from = names[slot];
  if (from === undefined) return null;
  let r0 = -1;
  let c0 = -1;
  p.areas.forEach((row, r) =>
    row.forEach((n, c) => {
      if (n === from && r0 < 0) {
        r0 = r;
        c0 = c;
      }
    }),
  );
  const dr = dir === 'up' ? -1 : dir === 'down' ? 1 : 0;
  const dc = dir === 'left' ? -1 : dir === 'right' ? 1 : 0;
  // Walk to the edge of the tile first, so a tile two cells wide does not land on itself.
  let r = r0;
  let c = c0;
  while (p.areas[r]?.[c] === from) {
    r += dr;
    c += dc;
  }
  const to = p.areas[r]?.[c];
  return to === undefined ? null : names.indexOf(to);
}

/** Screens at least this wide get the "desktop" starting preset, the rest the "tablet" one. */
export const DESKTOP_MIN_W = 1500;

export const presetClass = (width: number): 'desktop' | 'tablet' => (width >= DESKTOP_MIN_W ? 'desktop' : 'tablet');
