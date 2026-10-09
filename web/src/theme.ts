// Theme engine. A theme is a small base palette; style.css derives the rest of the ramp (surfaces,
// borders, tints) from it with color-mix, so a new theme is ~20 values and stays internally
// consistent. This module only touches the DOM inside functions so tests can import it.

export interface Theme {
  id: string;
  name: string;
  scheme: 'dark' | 'light';
  bg: string;
  fg: string;
  /** The app accent when no agent is in charge (login, empty state). */
  accent: string;
  ok: string;
  warn: string;
  danger: string;
  /** Accent while a Claude / Codex session is open. */
  claude: string;
  codex: string;
  /** black, red, green, yellow, blue, magenta, cyan, white, then the bright eight. */
  ansi: [
    string, string, string, string, string, string, string, string,
    string, string, string, string, string, string, string, string,
  ];
}

export const THEMES: Theme[] = [
  {
    id: 'midnight', name: 'Midnight', scheme: 'dark',
    bg: '#0b0d10', fg: '#d7dae0', accent: '#7aa2f7', ok: '#9ece6a', warn: '#e0af68', danger: '#f7768e',
    claude: '#e8a06b', codex: '#5ec4b6',
    ansi: ['#15161e', '#f7768e', '#9ece6a', '#e0af68', '#7aa2f7', '#bb9af7', '#7dcfff', '#a9b1d6',
      '#414868', '#ff899d', '#9fe044', '#faba4a', '#8db0ff', '#c7a9ff', '#a4daff', '#c0caf5'],
  },
  {
    id: 'tokyo-night', name: 'Tokyo Night', scheme: 'dark',
    bg: '#1a1b26', fg: '#c0caf5', accent: '#7aa2f7', ok: '#9ece6a', warn: '#e0af68', danger: '#f7768e',
    claude: '#ff9e64', codex: '#73daca',
    ansi: ['#15161e', '#f7768e', '#9ece6a', '#e0af68', '#7aa2f7', '#bb9af7', '#7dcfff', '#a9b1d6',
      '#414868', '#ff899d', '#9fe044', '#faba4a', '#8db0ff', '#c7a9ff', '#a4daff', '#c0caf5'],
  },
  {
    id: 'catppuccin-mocha', name: 'Catppuccin Mocha', scheme: 'dark',
    bg: '#1e1e2e', fg: '#cdd6f4', accent: '#89b4fa', ok: '#a6e3a1', warn: '#f9e2af', danger: '#f38ba8',
    claude: '#fab387', codex: '#94e2d5',
    ansi: ['#45475a', '#f38ba8', '#a6e3a1', '#f9e2af', '#89b4fa', '#f5c2e7', '#94e2d5', '#bac2de',
      '#585b70', '#f38ba8', '#a6e3a1', '#f9e2af', '#89b4fa', '#f5c2e7', '#94e2d5', '#a6adc8'],
  },
  {
    id: 'rose-pine', name: 'Rosé Pine', scheme: 'dark',
    bg: '#191724', fg: '#e0def4', accent: '#c4a7e7', ok: '#9ccfd8', warn: '#f6c177', danger: '#eb6f92',
    claude: '#ebbcba', codex: '#9ccfd8',
    ansi: ['#26233a', '#eb6f92', '#31748f', '#f6c177', '#9ccfd8', '#c4a7e7', '#ebbcba', '#e0def4',
      '#6e6a86', '#eb6f92', '#31748f', '#f6c177', '#9ccfd8', '#c4a7e7', '#ebbcba', '#e0def4'],
  },
  {
    id: 'gruvbox-dark', name: 'Gruvbox', scheme: 'dark',
    bg: '#282828', fg: '#ebdbb2', accent: '#83a598', ok: '#b8bb26', warn: '#fabd2f', danger: '#fb4934',
    claude: '#fe8019', codex: '#8ec07c',
    ansi: ['#282828', '#cc241d', '#98971a', '#d79921', '#458588', '#b16286', '#689d6a', '#a89984',
      '#928374', '#fb4934', '#b8bb26', '#fabd2f', '#83a598', '#d3869b', '#8ec07c', '#ebdbb2'],
  },
  {
    id: 'nord', name: 'Nord', scheme: 'dark',
    bg: '#2e3440', fg: '#d8dee9', accent: '#88c0d0', ok: '#a3be8c', warn: '#ebcb8b', danger: '#bf616a',
    claude: '#d08770', codex: '#81a1c1',
    ansi: ['#3b4252', '#bf616a', '#a3be8c', '#ebcb8b', '#81a1c1', '#b48ead', '#88c0d0', '#e5e9f0',
      '#4c566a', '#bf616a', '#a3be8c', '#ebcb8b', '#81a1c1', '#b48ead', '#8fbcbb', '#eceff4'],
  },
  {
    id: 'high-contrast', name: 'High contrast', scheme: 'dark',
    bg: '#000000', fg: '#ffffff', accent: '#5cc8ff', ok: '#3ff23f', warn: '#ffd23f', danger: '#ff5c5c',
    claude: '#ffa05c', codex: '#5cffd8',
    ansi: ['#000000', '#ff5c5c', '#3ff23f', '#ffd23f', '#5c9cff', '#ff7cf0', '#5cffe8', '#e6e6e6',
      '#8a8a8a', '#ff8080', '#7dff7d', '#ffe680', '#8fbaff', '#ffa6f6', '#99fff2', '#ffffff'],
  },
  {
    id: 'daylight', name: 'Daylight', scheme: 'light',
    bg: '#f6f7f9', fg: '#1f2430', accent: '#2f5fd0', ok: '#2d7a2d', warn: '#8f5f00', danger: '#c4263f',
    claude: '#b4501a', codex: '#0b7285',
    ansi: ['#1f2430', '#c4263f', '#2d7a2d', '#8f5f00', '#2f5fd0', '#8a3fb5', '#0b7285', '#a8adb8',
      '#5c6370', '#e0475f', '#3f9b3f', '#b87d00', '#4c7ae6', '#a35ccf', '#1e95ab', '#d0d3da'],
  },
  {
    id: 'catppuccin-latte', name: 'Catppuccin Latte', scheme: 'light',
    bg: '#eff1f5', fg: '#4c4f69', accent: '#1e66f5', ok: '#388a24', warn: '#a8670f', danger: '#d20f39',
    claude: '#c2410c', codex: '#137a80',
    ansi: ['#5c5f77', '#d20f39', '#40a02b', '#df8e1d', '#1e66f5', '#ea76cb', '#179299', '#acb0be',
      '#6c6f85', '#d20f39', '#40a02b', '#df8e1d', '#1e66f5', '#ea76cb', '#179299', '#bcc0cc'],
  },
];

export const DEFAULT_DARK = 'midnight';
export const DEFAULT_LIGHT = 'daylight';

export interface ThemePref {
  /** Follow the system light/dark setting, using `light` and `dark`. */
  auto: boolean;
  /** The theme used when `auto` is off. */
  theme: string;
  light: string;
  dark: string;
}

export const DEFAULT_PREF: ThemePref = { auto: false, theme: DEFAULT_DARK, light: DEFAULT_LIGHT, dark: DEFAULT_DARK };

export function themeById(id: string): Theme | undefined {
  return THEMES.find((t) => t.id === id);
}

/** The theme a preference means right now. Unknown ids fall back to the defaults. */
export function resolveTheme(pref: ThemePref, systemDark: boolean): Theme {
  const id = pref.auto ? (systemDark ? pref.dark : pref.light) : pref.theme;
  return themeById(id) ?? themeById(systemDark ? DEFAULT_DARK : DEFAULT_LIGHT)!;
}

// ---- colour maths (WCAG 2) ----------------------------------------------------------------

function rgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function luminance(hex: string): number {
  const [r, g, b] = rgb(hex).map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
}

export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

/** `pct` percent of `over` on top of `base`, like CSS color-mix(in srgb, base, over pct%). */
export function mix(base: string, over: string, pct: number): string {
  const a = rgb(base);
  const b = rgb(over);
  const out = a.map((v, i) => Math.round(v * (1 - pct / 100) + b[i]! * (pct / 100)));
  return '#' + out.map((v) => v.toString(16).padStart(2, '0')).join('');
}

/** How much of fg shows through in muted text; light backgrounds need more to keep contrast. */
export function mutedPct(t: Theme): number {
  return t.scheme === 'light' ? 85 : 65;
}

/** Text colour for filled buttons: whichever of near-black / white reads best on all three accents. */
export function onAccent(t: Theme): string {
  const dark = '#0b0d10';
  const light = '#ffffff';
  const worst = (c: string) => Math.min(...[t.accent, t.claude, t.codex].map((a) => contrast(c, a)));
  return worst(dark) >= worst(light) ? dark : light;
}

// ---- applying ------------------------------------------------------------------------------

const PREF_KEY = 'ttm.theme';
const BG_KEY = 'ttm.theme.bg';

let pref: ThemePref = DEFAULT_PREF;
let active: Theme = THEMES[0]!;
const listeners = new Set<(t: Theme) => void>();

export function currentTheme(): Theme {
  return active;
}

export function currentPref(): ThemePref {
  return pref;
}

/** Run `cb` whenever the active theme changes. Returns the unsubscribe. */
export function onThemeChange(cb: (t: Theme) => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

function systemDark(): boolean {
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? true;
}

function apply(t: Theme) {
  active = t;
  const root = document.documentElement;
  const vars: Record<string, string> = {
    '--bg': t.bg, '--fg': t.fg, '--accent-base': t.accent, '--ok': t.ok, '--warn': t.warn, '--danger': t.danger,
    '--claude': t.claude, '--codex': t.codex, '--on-accent': onAccent(t), '--muted-pct': mutedPct(t) + '%',
  };
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
  root.style.colorScheme = t.scheme;
  root.style.background = t.bg;
  root.dataset.theme = t.id;
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', t.bg);
  try {
    localStorage.setItem(BG_KEY, t.bg); // read by the inline script in index.html to avoid a flash
  } catch {
    /* private mode: the first paint may flash */
  }
  for (const cb of listeners) cb(t);
}

function load(): ThemePref {
  try {
    const raw = JSON.parse(localStorage.getItem(PREF_KEY) ?? 'null') as Partial<ThemePref> | null;
    if (raw && typeof raw === 'object') return { ...DEFAULT_PREF, ...raw };
  } catch {
    /* fall through to the default */
  }
  return DEFAULT_PREF;
}

/** Apply the saved choice and follow the system setting while Auto is on. Call once at startup. */
export function initTheme() {
  pref = load();
  apply(resolveTheme(pref, systemDark()));
  window.matchMedia?.('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if (pref.auto) apply(resolveTheme(pref, systemDark()));
  });
}

export function setPref(next: Partial<ThemePref>) {
  pref = { ...pref, ...next };
  try {
    localStorage.setItem(PREF_KEY, JSON.stringify(pref));
  } catch {
    /* applied for this page view only */
  }
  apply(resolveTheme(pref, systemDark()));
}

// ---- terminal ------------------------------------------------------------------------------

export const UI_FONT = "'Inter Variable', system-ui, sans-serif";
export const MONO_FALLBACK = 'ui-monospace, Menlo, monospace';
export const MONO_FONT = "'JetBrains Mono Variable', ui-monospace, Menlo, monospace";

/** The xterm palette for a theme. */
export function xtermTheme(t: Theme = active) {
  const [black, red, green, yellow, blue, magenta, cyan, white, bBlack, bRed, bGreen, bYellow, bBlue, bMagenta, bCyan, bWhite] =
    t.ansi;
  return {
    background: t.bg,
    foreground: t.fg,
    cursor: t.accent,
    cursorAccent: t.bg,
    selectionBackground: mix(t.bg, t.accent, 35),
    black, red, green, yellow, blue, magenta, cyan, white,
    brightBlack: bBlack, brightRed: bRed, brightGreen: bGreen, brightYellow: bYellow,
    brightBlue: bBlue, brightMagenta: bMagenta, brightCyan: bCyan, brightWhite: bWhite,
  };
}
