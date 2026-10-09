// A line diff for showing what an edit changed. Pure, so node can test it.

export type DiffLine = { t: 'ctx' | 'add' | 'del'; v: string };

/** Above this many cells the exact diff is skipped (every old line removed, every new one added). */
const MAX_CELLS = 250_000;

/**
 * Lines of `oldText` → `newText`. Shared leading and trailing lines are kept as context, and the
 * middle is diffed with a longest-common-subsequence table.
 */
export function lineDiff(oldText: string, newText: string): DiffLine[] {
  const a = oldText === '' ? [] : oldText.split('\n');
  const b = newText === '' ? [] : newText.split('\n');

  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;

  const midA = a.slice(head, a.length - tail);
  const midB = b.slice(head, b.length - tail);
  const out: DiffLine[] = a.slice(0, head).map((v) => ({ t: 'ctx', v }));

  if (midA.length * midB.length > MAX_CELLS) {
    out.push(...midA.map((v): DiffLine => ({ t: 'del', v })), ...midB.map((v): DiffLine => ({ t: 'add', v })));
  } else {
    // lcs[i][j] = length of the longest common subsequence of midA[i..] and midB[j..]
    const w = midB.length + 1;
    const lcs = new Uint32Array((midA.length + 1) * w);
    for (let i = midA.length - 1; i >= 0; i--) {
      for (let j = midB.length - 1; j >= 0; j--) {
        lcs[i * w + j] =
          midA[i] === midB[j] ? lcs[(i + 1) * w + j + 1] + 1 : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < midA.length && j < midB.length) {
      if (midA[i] === midB[j]) {
        out.push({ t: 'ctx', v: midA[i] });
        i++;
        j++;
      } else if (lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
        out.push({ t: 'del', v: midA[i++] });
      } else {
        out.push({ t: 'add', v: midB[j++] });
      }
    }
    while (i < midA.length) out.push({ t: 'del', v: midA[i++] });
    while (j < midB.length) out.push({ t: 'add', v: midB[j++] });
  }

  out.push(...a.slice(a.length - tail).map((v): DiffLine => ({ t: 'ctx', v })));
  return out;
}

/** Keep `radius` lines of context around changes and collapse the rest into one gap marker. */
export function trimContext(lines: DiffLine[], radius = 2): (DiffLine | { t: 'gap'; n: number })[] {
  const keep = new Array<boolean>(lines.length).fill(false);
  lines.forEach((l, i) => {
    if (l.t === 'ctx') return;
    for (let k = Math.max(0, i - radius); k <= Math.min(lines.length - 1, i + radius); k++) keep[k] = true;
  });
  const out: (DiffLine | { t: 'gap'; n: number })[] = [];
  let skipped = 0;
  lines.forEach((l, i) => {
    if (keep[i]) {
      if (skipped) out.push({ t: 'gap', n: skipped });
      skipped = 0;
      out.push(l);
    } else {
      skipped++;
    }
  });
  if (skipped) out.push({ t: 'gap', n: skipped });
  return out;
}
