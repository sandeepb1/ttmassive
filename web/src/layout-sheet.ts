// The layout sheet: pick a pane grid, and save, open or delete saved layouts (a grid plus the
// session in each tile). Opened from the grid button in a pane's header.

import { el } from './dom';
import { warn } from './haptics';
import { Sheet } from './sheet';
import { icon } from './icons';
import { button } from './ui/button';
import { input as textInput } from './ui/input';
import { row as listRow } from './ui/row';
import { sheetBlock, sheetHint, sheetLabel } from './ui/sheet-parts';
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
  const box = el('div', 'grid h-12 w-18 gap-0.5');
  box.style.gridTemplateAreas = p.areas.map((r) => `"${r.join(' ')}"`).join(' ');
  box.style.gridTemplateColumns = p.cols.map((c) => `${c}fr`).join(' ');
  box.style.gridTemplateRows = p.rows.map((r) => `${r}fr`).join(' ');
  for (const n of tileNames(p)) {
    const cell = el('span', 'rounded-xs bg-faint');
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
  private note = sheetHint('');
  private list = sheetBlock();

  constructor(
    host: HTMLElement,
    private hooks: LayoutHooks,
  ) {
    super(host, '', 'Layout', hooks.onUnauthorized);
  }

  open() {
    const usable = new Set(this.hooks.usable());
    const active = this.hooks.active();
    const grid = el('div', 'grid grid-cols-[repeat(auto-fill,minmax(6.5rem,1fr))] gap-2');
    for (const p of PRESETS) {
      const b = el('button', 'flex touch-manipulation flex-col items-center gap-1.5 rounded-lg bg-muted px-1.5 py-2.5 text-foreground aria-pressed:bg-accent-bg aria-pressed:ring-2 aria-pressed:ring-primary disabled:opacity-40');
      b.type = 'button';
      b.disabled = !usable.has(p.id);
      b.setAttribute('aria-pressed', String(p.id === active));
      b.append(thumb(p.id), el('span', 'text-center text-xs text-muted-foreground', p.label));
      b.addEventListener('click', () => {
        this.hooks.choose(p.id);
        this.close();
      });
      grid.append(b);
    }
    const presets = sheetBlock(sheetLabel('Grid'), grid, sheetHint('Greyed-out grids do not fit this screen or the pane limit in Settings.'));

    const save = el('form', 'flex gap-2');
    const name = textInput({ placeholder: 'Name this layout, e.g. Claude + Codex', label: 'Layout name', cls: 'flex-1' });
    name.maxLength = 40;
    const go = button({}, 'Save');
    go.type = 'submit';
    save.append(name, go);
    save.addEventListener('submit', (e) => {
      e.preventDefault();
      void this.save(name.value.trim(), name);
    });

    const ws = sheetBlock(sheetLabel('Saved layouts'), save, this.list);
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
    if (!next) return this.say('Could not save (too many saved layouts, or the server refused)');
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
      const line = el('div', 'flex items-stretch gap-1.5');
      const open = listRow(w.name, { hint: summary(w), cls: 'min-w-0 flex-1' });
      open.addEventListener('click', () => {
        this.hooks.open(w);
        this.close();
      });
      const del = button({ variant: 'secondary', size: 'icon', label: `Delete ${w.name}`, cls: 'h-auto', onClick: () => { warn(); void this.remove(w.name); } }, icon('close'));
      line.append(open, del);
      return line;
    });
    this.list.replaceChildren(...(rows.length ? rows : [sheetHint('None saved yet.')]));
  }

  private say(text: string) {
    this.note.textContent = text;
  }
}
