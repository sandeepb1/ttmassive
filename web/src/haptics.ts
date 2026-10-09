// A short buzz for the few actions that deserve one (Android only: iOS has no vibrate API).

function buzz(pattern: number | number[]) {
  try {
    navigator.vibrate?.(pattern);
  } catch {
    /* not allowed here */
  }
}

/** A light tap: sending or stopping. */
export const tap = () => buzz(10);

/** A firmer double pulse, for a confirm that throws something away. */
export const warn = () => buzz([18, 40, 18]);
