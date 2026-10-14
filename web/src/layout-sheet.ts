// The layout sheet: pick a pane grid, and save, open or delete workspaces (a grid plus the
// session in each tile). Opened from the grid button in a pane's header.

import { el } from './dom';
import { warn } from './haptics';
import { Sheet } from './sheet';
import { PRESETS, preset, tileNames, type PresetId } from './tiling';
import { updateLayouts, type Layouts, type Workspace } from './workspace';

export interface LayoutHooks {
  onUnauthorized: () => void;
  /** The preset on screen now. */
  active: () => PresetId;
  /** Presets that fit this screen and the pane limit. */
  usable: () => PresetId[];
  choose: (id: PresetId) => void;
  /** The grid and sessions on screen, as a workspace called `name`. */
  snapshot: (name: string) => Workspace;
  open: (w: Workspace) => void;
  workspaces: () => Workspace[];
  /** The stored document changed (saved or deleted). */
  saved: (l: Layouts) => void;
}

/** A small picture of a preset: its tiles as boxes. */
function thumb(id: PresetId): HTMLElement {
  const p = preset(id);
  const box = el('div', 'lp-thumb');
  box.style.gridTemplateAreas = p.areas.map((r) => `"${r.join(' ')}"`).join(' ');
  box.style.gridTemplateColumns = p.cols.map((c) => `${c}fr`).join(' ');
  box.style.gridTemplateRows = p.rows.map((r) => `${r}fr`).join(' ');
  for (const n of tileNames(p)) {
    const cell = el('span');
    cell.style.gridArea = n;
    box.append(cell);
  }
  return box;
}

const summary = (w: Workspace) => {
  const named = w.tiles.filter(Boolean);
  const grid = preset(w.preset).label;
  return named.length ? `${grid} · ${named.join(', ')}` : grid;
};

export class LayoutSheet extends Sheet {
  private note = el('div', 'sheet-hint');
  private list = el('div', 'more-block');

  constructor(
    host: HTMLElement,
    private hooks: LayoutHooks,
  ) {
    super(host, '', 'Layout', hooks.onUnauthorized);
  }

  open() {
    const usable = new Set(this.hooks.usable());
    const active = this.hooks.active();
    const grid = el('div', 'lp-grid');
    for (const p of PRESETS) {
      const b = el('button', 'lp-preset');
      b.type = 'button';
      b.disabled = !usable.has(p.id);
      b.setAttribute('aria-pressed', String(p.id === active));
      b.append(thumb(p.id), el('span', 'lp-label', p.label));
      b.addEventListener('click', () => {
        this.hooks.choose(p.id);
        this.close();
      });
      grid.append(b);
    }
    const presets = el('div', 'more-block');
    presets.append(el('div', 'sheet-label', 'Grid'), grid, el('div', 'sheet-hint', 'Greyed-out grids do not fit this screen or the pane limit in Settings.'));

    const save = el('form', 'ws-save');
    const name = el('input', 'ws-name');
    name.type = 'text';
    name.maxLength = 40;
    name.placeholder = 'Name this layout, e.g. Claude + Codex';
    name.setAttribute('aria-label', 'Workspace name');
    const go = el('button', 'chip chip-go', 'Save');
    go.type = 'submit';
    save.append(name, go);
    save.addEventListener('submit', (e) => {
      e.preventDefault();
      void this.save(name.value.trim(), name);
    });

    const ws = el('div', 'more-block');
    ws.append(el('div', 'sheet-label', 'Workspaces'), save, this.list);
    this.renderList();
    this.body.replaceChildren(presets, ws, this.note);
    this.show();
  }

  private async save(name: string, input: HTMLInputElement) {
    if (!name) return this.say('Give it a name first');
    const w = this.hooks.snapshot(name);
    const next = await updateLayouts((l) => ({
      ...l,
      workspaces: [...l.workspaces.filter((x) => x.name !== name), w],
    }));
    if (!next) return this.say('Could not save (too many workspaces, or the server refused)');
    this.hooks.saved(next);
    input.value = '';
    this.say(`Saved “${name}”`);
    this.renderList();
  }

  private async remove(name: string) {
    const next = await updateLayouts((l) => ({ ...l, workspaces: l.workspaces.filter((x) => x.name !== name) }));
    if (!next) return this.say('Could not delete');
    this.hooks.saved(next);
    this.renderList();
  }

  private renderList() {
    const rows = this.hooks.workspaces().map((w) => {
      const row = el('div', 'ws-row');
      const open = el('button', 'ws-open');
      open.type = 'button';
      open.append(el('span', 'ws-title', w.name), el('span', 'ws-sum', summary(w)));
      open.addEventListener('click', () => {
        this.hooks.open(w);
        this.close();
      });
      const del = el('button', 'ws-del', '×');
      del.type = 'button';
      del.setAttribute('aria-label', `Delete ${w.name}`);
      del.addEventListener('click', () => {
        warn();
        void this.remove(w.name);
      });
      row.append(open, del);
      return row;
    });
    this.list.replaceChildren(...(rows.length ? rows : [el('div', 'sheet-hint', 'None saved yet.')]));
  }

  private say(text: string) {
    this.note.textContent = text;
  }
}
