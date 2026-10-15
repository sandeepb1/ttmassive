// The fonts the terminal can use. JetBrains Mono ships with the page; the others are separate
// files fetched the first time they are chosen. A font installed on the device can be named instead.
// No DOM at import time, so the stack-building is unit-testable.

import { MONO_FONT } from './theme.ts';

export interface TermFont {
  id: string;
  label: string;
  /** The CSS family name once loaded; null for the system and custom entries. */
  family: string | null;
  /** The font draws ligatures for `->`, `!=` and the like. */
  ligatures: boolean;
  /** Fetch the font's files. Absent for fonts already on the page. */
  load?: () => Promise<unknown>;
}

export const TERM_FONTS: TermFont[] = [
  { id: 'jetbrains', label: 'JetBrains Mono', family: 'JetBrains Mono Variable', ligatures: true },
  { id: 'fira', label: 'Fira Code', family: 'Fira Code Variable', ligatures: true, load: () => import('@fontsource-variable/fira-code/wght.css') },
  { id: 'cascadia', label: 'Cascadia Code', family: 'Cascadia Code Variable', ligatures: true, load: () => import('@fontsource-variable/cascadia-code/wght.css') },
  { id: 'geist', label: 'Geist Mono', family: 'Geist Mono Variable', ligatures: true, load: () => import('@fontsource-variable/geist-mono/wght.css') },
  { id: 'source', label: 'Source Code Pro', family: 'Source Code Pro Variable', ligatures: false, load: () => import('@fontsource-variable/source-code-pro/wght.css') },
  { id: 'system', label: 'System monospace', family: null, ligatures: false },
  { id: 'custom', label: 'Installed font…', family: null, ligatures: true },
];

const SYSTEM_STACK = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';

export const termFont = (id: string): TermFont => TERM_FONTS.find((f) => f.id === id) ?? TERM_FONTS[0]!;

/** A family name typed by the user, made safe to put inside CSS quotes. */
export function cleanFamily(raw: string): string {
  return raw.replace(/["'\;{}<>]/g, '').replace(/\s+/g, ' ').trim().slice(0, 64);
}

/** The font-family value for xterm: the chosen font first, then a monospace that is always there. */
export function fontStack(id: string, custom: string): string {
  const f = termFont(id);
  if (f.id === 'system') return SYSTEM_STACK;
  if (f.id === 'custom') {
    const name = cleanFamily(custom);
    return name ? `"${name}", ${MONO_FONT}` : MONO_FONT;
  }
  return `"${f.family}", ${SYSTEM_STACK}`;
}

/** Whether the choice can draw ligatures at all. */
export function hasLigatures(id: string, custom: string): boolean {
  const f = termFont(id);
  return f.id === 'custom' ? cleanFamily(custom) !== '' : f.ligatures;
}

/** Make the font ready to draw with, then give back the stack to use. A font that cannot load keeps the fallbacks. */
export async function prepareFont(id: string, custom: string): Promise<string> {
  const f = termFont(id);
  const family = f.id === 'custom' ? cleanFamily(custom) : f.family;
  try {
    await f.load?.();
    if (family) await document.fonts?.load(`14px "${family}"`);
  } catch {
    /* offline, or the font is not installed: the stack's fallbacks draw instead */
  }
  return fontStack(id, custom);
}

/**
 * The ligatures addon asks the browser for the list of installed fonts so it can read their ligature tables.
 * That raises a permission prompt in Chrome, and it can never see the fonts bundled with the page. Refuse the
 * question once for the whole page; the addon then uses its built-in list of ligatures, which suits the fonts here.
 */
export function blockLocalFontPrompt() {
  const refuse = () => Promise.reject(new DOMException('Not asked: see term-fonts.ts', 'NotAllowedError'));
  if ('queryLocalFonts' in window) (window as unknown as { queryLocalFonts: unknown }).queryLocalFonts = refuse;
  const fonts = (navigator as unknown as { fonts?: { query?: unknown } }).fonts;
  if (fonts && 'query' in fonts) fonts.query = refuse;
}
