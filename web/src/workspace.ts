// Pane preferences and saved workspaces. They live on the server (`/api/layouts`) so a layout
// saved on one device opens on another. A bad or missing document never breaks the page: the
// parser falls back to defaults field by field.

import { DEFAULT_PRESET, isPresetId, preset, sanitizeSizes, type PresetId } from './tiling.ts';

export interface LayoutSettings {
  /** Allow more than one pane at a time. */
  multi: boolean;
  maxPanes: number;
  /** The preset a second pane starts from on a big screen and on a tablet-sized one. */
  desktop: PresetId;
  tablet: PresetId;
}

export interface Workspace {
  name: string;
  preset: PresetId;
  cols: number[];
  rows: number[];
  /** Session in each tile, in tile order; '' for an empty tile. */
  tiles: string[];
}

export interface Layouts {
  settings: LayoutSettings;
  workspaces: Workspace[];
  /** Project folders kept in the session list even with no session in them. */
  pinnedProjects: string[];
}

export const MAX_PANES_LIMIT = 6;
const SESSION_ID = /^[A-Za-z0-9_-]{1,64}$/;

export const defaultLayouts = (): Layouts => ({
  settings: { multi: true, maxPanes: 4, desktop: DEFAULT_PRESET, tablet: DEFAULT_PRESET },
  workspaces: [],
  pinnedProjects: [],
});

const isObj = (x: unknown): x is Record<string, unknown> => typeof x === 'object' && x !== null && !Array.isArray(x);

/** Read a document from the server, keeping whatever in it is usable. */
export function parseLayouts(raw: unknown): Layouts {
  const out = defaultLayouts();
  if (!isObj(raw)) return out;
  const s = raw.settings;
  if (isObj(s)) {
    if (typeof s.multi === 'boolean') out.settings.multi = s.multi;
    if (typeof s.maxPanes === 'number' && Number.isInteger(s.maxPanes)) {
      out.settings.maxPanes = Math.min(MAX_PANES_LIMIT, Math.max(1, s.maxPanes));
    }
    if (isPresetId(s.desktop)) out.settings.desktop = s.desktop;
    if (isPresetId(s.tablet)) out.settings.tablet = s.tablet;
  }
  if (Array.isArray(raw.workspaces)) {
    for (const w of raw.workspaces) {
      if (!isObj(w) || typeof w.name !== 'string' || !w.name.trim() || !isPresetId(w.preset)) continue;
      const p = preset(w.preset);
      const sizes = sanitizeSizes(p, w.cols, w.rows);
      const tiles = Array.isArray(w.tiles)
        ? w.tiles.filter((t): t is string => typeof t === 'string' && (t === '' || SESSION_ID.test(t))).slice(0, MAX_PANES_LIMIT)
        : [];
      out.workspaces.push({ name: w.name, preset: w.preset, cols: sizes.cols, rows: sizes.rows, tiles });
    }
  }
  if (Array.isArray(raw.pinnedProjects)) {
    for (const p of raw.pinnedProjects) {
      if (typeof p === 'string' && p.trim() && !/[\u0000-\u001f]/.test(p) && !out.pinnedProjects.includes(p)) out.pinnedProjects.push(p);
    }
  }
  return out;
}

/** How many panes the settings allow right now. */
export const paneLimit = (s: LayoutSettings) => (s.multi ? s.maxPanes : 1);

export async function fetchLayouts(): Promise<Layouts> {
  try {
    const r = await fetch('/api/layouts', { cache: 'no-store' });
    return r.ok ? parseLayouts(await r.json()) : defaultLayouts();
  } catch {
    return defaultLayouts();
  }
}

/**
 * Change the stored document: read the latest (another device may have saved since this page
 * loaded), apply `edit`, write it back. Returns the saved document, or null if it was refused.
 */
export async function updateLayouts(edit: (l: Layouts) => Layouts): Promise<Layouts | null> {
  const next = edit(await fetchLayouts());
  try {
    const r = await fetch('/api/layouts', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(next),
    });
    return r.ok ? next : null;
  } catch {
    return null;
  }
}
