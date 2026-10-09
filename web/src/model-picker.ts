// The model and effort picker: a sheet over the chat that mirrors the agent's own `/model` menu
// (Claude Code or Codex). The list comes from the agent's real menu, so it is always the models
// it offers now. The server drives the menu with "this session only" keys, so your default for
// new sessions is never changed from here. Codex asks for a reasoning level after the model, so
// its picker has a second step.

import { levelKey } from './chat-model';
import { el } from './dom';

interface ModelRow {
  number: number;
  label: string;
  description: string;
  current: boolean;
  /** Codex marks the model or level a new session starts with. */
  default: boolean;
}

interface ModelMenu {
  rows: ModelRow[];
  effort: string | null;
}

/** Codex's second screen: the reasoning levels of the model just picked. */
interface LevelMenu {
  model: string;
  rows: ModelRow[];
}

/** The levels each agent offers; the server checks the one you pick against its own menu. */
const EFFORT_LEVELS = {
  claude: ['low', 'medium', 'high', 'xhigh', 'max'],
  codex: ['low', 'medium', 'high', 'xhigh'],
} as const;



export interface PickerHooks {
  onUnauthorized: () => void;
  /** The change went through. `message` is what Claude said, such as "Set model to Haiku 5.5…". */
  onChanged: (change: { model?: string; effort?: string }, message: string) => void;
}

export class ModelPicker {
  private backdrop = el('div', 'sheet-backdrop');
  private sheet = el('div', 'sheet');
  private body = el('div', 'sheet-body');
  /** Claude's own menu is open on the host and still needs closing. */
  private menuOpen = false;
  private busy = false;
  private gone = true;

  constructor(
    private host: HTMLElement,
    private session: string,
    private agent: 'claude' | 'codex',
    private hooks: PickerHooks,
  ) {
    this.backdrop.addEventListener('click', () => this.close());
    // A tap on a button must not take focus from the message box: that would drop the on-screen
    // keyboard and make the sheet jump under the next tap.
    this.sheet.addEventListener('pointerdown', (e) => {
      if (e.target instanceof Element && e.target.closest('button')) e.preventDefault();
    });
    const head = el('div', 'sheet-head');
    head.append(el('strong', undefined, 'Model'));
    const x = el('button', 'sheet-x', '×');
    x.type = 'button';
    x.setAttribute('aria-label', 'Close');
    x.addEventListener('click', () => this.close());
    head.append(x);
    this.sheet.append(head, this.body);
  }

  private url(path: string) {
    return `/api/sessions/${encodeURIComponent(this.session)}/${path}`;
  }

  private async post(path: string, body?: unknown): Promise<Response | null> {
    const r = await fetch(this.url(path), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    }).catch(() => null);
    if (r?.status === 401) this.hooks.onUnauthorized();
    return r;
  }

  async open(currentEffort: string | null) {
    if (!this.gone) return;
    this.gone = false;
    this.host.append(this.backdrop, this.sheet);
    this.showMessage('Opening the model menu…');
    const r = await this.post('model/open');
    if (this.gone) return; // closed while we waited
    if (!r) return this.showMessage('Server unreachable', true);
    if (!r.ok) return this.showMessage((await r.text()) || 'Could not open the model menu', true);
    this.menuOpen = true;
    this.renderMenu((await r.json()) as ModelMenu, currentEffort);
  }

  /** Close the sheet, and Claude's menu behind it if it is still open. */
  close() {
    if (this.gone) return;
    this.gone = true;
    this.backdrop.remove();
    this.sheet.remove();
    if (this.menuOpen) {
      this.menuOpen = false;
      void this.post('model/close');
    }
  }

  private showMessage(text: string, isError = false) {
    this.body.replaceChildren(el('div', isError ? 'sheet-msg err' : 'sheet-msg', text));
  }

  private renderMenu(menu: ModelMenu, currentEffort: string | null) {
    const list = el('div', 'sheet-list');
    for (const row of menu.rows) {
      const b = el('button', row.current ? 'sheet-row current' : 'sheet-row');
      b.type = 'button';
      const top = el('span', 'sr-top');
      top.append(el('span', 'sr-label', row.label));
      if (row.current) top.append(el('span', 'sr-mark', '✓ in use'));
      b.append(top);
      if (row.description) b.append(el('span', 'sr-desc', row.description));
      b.addEventListener('click', () => void this.choose(row));
      list.append(b);
    }

    const effort = el('div', 'sheet-effort');
    effort.append(el('div', 'sheet-label', 'Effort'));
    const pills = el('div', 'pills');
    for (const level of EFFORT_LEVELS[this.agent]) {
      const p = el('button', level === currentEffort ? 'pill on' : 'pill', level);
      p.type = 'button';
      p.addEventListener('click', () => void this.setEffort(level));
      pills.append(p);
    }
    effort.append(pills, el('div', 'sheet-hint', 'Applies to this session only.'));

    this.body.replaceChildren(
      el('div', 'sheet-hint', 'Applies to this session only. Your default for new sessions is not changed.'),
      list,
      effort,
    );
  }

  private async choose(row: ModelRow) {
    if (this.busy) return;
    this.busy = true;
    this.showMessage(`Switching to ${row.label}…`);
    const r = await this.post('model/choose', { number: row.number });
    this.busy = false;
    if (!r) {
      this.menuOpen = false;
      return this.showMessage('Server unreachable', true);
    }
    if (!r.ok) {
      this.menuOpen = false; // the server closes the menu when something goes wrong
      return this.showMessage((await r.text()) || 'Could not switch the model', true);
    }
    const body = (await r.json()) as { message?: string; levels?: LevelMenu };
    // Codex does not apply the model yet: it wants a reasoning level first.
    if (body.levels) return this.renderLevels(row, body.levels);
    this.menuOpen = false;
    this.finish({ model: row.label }, body.message ?? 'Model changed');
  }

  /** Codex's second step: pick how hard the new model should think. */
  private renderLevels(model: ModelRow, menu: LevelMenu) {
    const list = el('div', 'sheet-list');
    // A row ending in an ellipsis ("More reasoning…") opens yet another menu rather than choosing.
    for (const row of menu.rows.filter((r) => !r.label.endsWith('…'))) {
      const b = el('button', 'sheet-row');
      b.type = 'button';
      const top = el('span', 'sr-top');
      top.append(el('span', 'sr-label', row.label));
      if (row.default) top.append(el('span', 'sr-mark', 'default'));
      b.append(top);
      if (row.description) b.append(el('span', 'sr-desc', row.description));
      b.addEventListener('click', () => void this.chooseLevel(model, row));
      list.append(b);
    }
    this.body.replaceChildren(
      el('div', 'sheet-label', `Reasoning level for ${menu.model}`),
      el('div', 'sheet-hint', 'Applies to this session only.'),
      list,
    );
  }

  private async chooseLevel(model: ModelRow, row: ModelRow) {
    if (this.busy) return;
    this.busy = true;
    this.showMessage(`Switching to ${model.label} · ${row.label}…`);
    const r = await this.post('model/level', { number: row.number });
    this.busy = false;
    this.menuOpen = false;
    if (!r) return this.showMessage('Server unreachable', true);
    if (!r.ok) return this.showMessage((await r.text()) || 'Could not switch the model', true);
    const { message } = (await r.json()) as { message: string };
    this.finish({ model: model.label, effort: levelKey(row.label) }, message);
  }

  private async setEffort(level: string) {
    if (this.busy) return;
    this.busy = true;
    this.showMessage(`Setting effort to ${level}…`);
    const r = await this.post('effort', { level });
    this.busy = false;
    this.menuOpen = false;
    if (!r) return this.showMessage('Server unreachable', true);
    if (!r.ok) return this.showMessage((await r.text()) || 'Could not set the effort', true);
    const { message } = (await r.json()) as { message: string };
    this.finish({ effort: level }, message);
  }

  private finish(change: { model?: string; effort?: string }, message: string) {
    this.close();
    this.hooks.onChanged(change, message);
  }
}
