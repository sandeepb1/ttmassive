// Theme chooser: swatch cards that preview each theme in its own colours, plus an Auto switch that
// follows the system light/dark setting with one theme for each.

import { el } from './dom';
import { currentPref, setPref, THEMES, type Theme } from './theme';

export class ThemePicker {
  readonly root = el('div', 'theme-picker');

  constructor() {
    this.render();
  }

  private render() {
    const pref = currentPref();
    const label = el('div', 'drawer-label', 'Theme');

    const auto = el('button', 'chip', pref.auto ? 'Auto: follows system ✓' : 'Auto: follow system');
    auto.type = 'button';
    auto.setAttribute('aria-pressed', String(pref.auto));
    auto.addEventListener('click', () => {
      setPref({ auto: !pref.auto });
      this.render();
    });

    const body = el('div', 'theme-sections');
    if (pref.auto) {
      body.append(
        this.section('Light', THEMES.filter((t) => t.scheme === 'light'), pref.light, (id) => setPref({ light: id })),
        this.section('Dark', THEMES.filter((t) => t.scheme === 'dark'), pref.dark, (id) => setPref({ dark: id })),
      );
    } else {
      body.append(this.section(null, THEMES, pref.theme, (id) => setPref({ theme: id })));
    }
    this.root.replaceChildren(label, auto, body);
  }

  private section(title: string | null, themes: Theme[], selected: string, pick: (id: string) => void): HTMLElement {
    const sec = el('div', 'theme-section');
    if (title) sec.append(el('div', 'theme-sub', title));
    const grid = el('div', 'theme-grid');
    for (const t of themes) {
      const card = el('button', 'theme-card');
      card.type = 'button';
      card.style.background = t.bg;
      card.style.color = t.fg;
      card.setAttribute('aria-pressed', String(t.id === selected));
      card.setAttribute('aria-label', `${t.name} theme`);
      const dots = el('span', 'theme-dots');
      for (const c of [t.accent, t.ok, t.warn, t.danger]) {
        const d = el('i');
        d.style.background = c;
        dots.append(d);
      }
      card.append(el('span', 'theme-name', t.name), dots);
      card.addEventListener('click', () => {
        pick(t.id);
        this.render();
      });
      grid.append(card);
    }
    sec.append(grid);
    return sec;
  }
}
