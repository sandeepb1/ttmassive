// Interface text size: scales the root font size, so everything set in rem follows.

const KEY = 'ttm.uiScale';
export const SCALE_MIN = 85;
export const SCALE_MAX = 140;
export const SCALE_STEP = 5;
export const SCALE_DEFAULT = 100;

export function clampScale(pct: number): number {
  if (!Number.isFinite(pct)) return SCALE_DEFAULT;
  return Math.min(SCALE_MAX, Math.max(SCALE_MIN, Math.round(pct / SCALE_STEP) * SCALE_STEP));
}

export function getUiScale(): number {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved !== null) return clampScale(Number(saved));
  } catch {
    /* storage unavailable */
  }
  return SCALE_DEFAULT;
}

export function setUiScale(pct: number) {
  const v = clampScale(pct);
  document.documentElement.style.fontSize = `${(16 * v) / 100}px`;
  try {
    localStorage.setItem(KEY, String(v));
  } catch {
    /* applied for this page view only */
  }
}

export function initUiScale() {
  document.documentElement.style.fontSize = `${(16 * getUiScale()) / 100}px`;
}
