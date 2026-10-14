// Finding file paths in terminal text, so they can be tapped. Agents and compilers print them
// everywhere: `src/main.rs:42:7`, `./lib/a.ts(12,3)`, `/etc/hosts`. The hard part is not matching
// too much: URLs, versions, `and/or`, and a slash command like `/model` are not files.

export interface PathMatch {
  /** Offsets into the text, `end` exclusive. They include any `:line:col` suffix. */
  start: number;
  end: number;
  /** The path itself, as printed. */
  path: string;
  line: number | null;
  col: number | null;
}

/** Extensions that make a bare name (no folder) believable as a file: `main.rs`, not `example.com`. */
const CODE_EXTS = new Set(
  ('rs ts tsx js jsx mjs cjs py go java kt kts c h cc cpp hpp cs rb php sh bash zsh fish md mdx toml json jsonc yaml yml ' +
    'css scss html htm xml sql lock txt cfg ini conf env log swift m mm lua zig ex exs erl hs ml vue svelte astro dart ' +
    'gradle proto graphql tf nix csv tsv').split(' '),
);

/** Names with no extension that are still files. */
const KNOWN_NAMES = new Set(['Makefile', 'Dockerfile', 'LICENSE', 'README', 'CHANGELOG', 'Gemfile', 'Rakefile', 'Procfile', 'Justfile']);

const RUN = /[\w@.~\-/]+/g;
/** A whole URL, port and all: nothing inside one is a file. */
const URL_SPAN = /[a-z][a-z0-9+.-]*:\/\/\S+/gi;
const COLON_SUFFIX = /^:(\d+)(?::(\d+))?/;
const PAREN_SUFFIX = /^\((\d+)(?:,\s?(\d+))?\)/;

function believable(run: string, hasLine: boolean): boolean {
  if (run.length < 2 || run.startsWith('-')) return false;
  const segments = run.split('/');
  const last = segments[segments.length - 1] ?? '';
  if (!last || !/[A-Za-z]/.test(last)) return false; // a folder, or only digits and dots: `1/2`, `3.14`
  const ext = /\.([A-Za-z0-9]{1,8})$/.exec(last)?.[1];
  const known = KNOWN_NAMES.has(last) || (last.startsWith('.') && last.length > 1 && !last.startsWith('..'));
  const named = ext !== undefined || known;
  if (!run.includes('/')) {
    // `main.rs` or `Makefile:3`, but not `example.com` or `e.g`.
    return (ext !== undefined && CODE_EXTS.has(ext.toLowerCase())) || (hasLine && named) || (known && hasLine);
  }
  if (named || hasLine) return true; // `src/main.rs`, `scripts/build:12`
  // No extension: only believable as an explicit location, never `and/or` or the `/model` command.
  if (run.startsWith('~/') || run.startsWith('./') || run.startsWith('../')) return true;
  return run.startsWith('/') && segments.filter(Boolean).length >= 2;
}

/** Every file path in `text`, with the line and column printed after it, if any. */
export function findPaths(text: string): PathMatch[] {
  const out: PathMatch[] = [];
  const urls = [...text.matchAll(URL_SPAN)].map((u) => [u.index ?? 0, (u.index ?? 0) + u[0].length] as const);
  for (const m of text.matchAll(RUN)) {
    let run = m[0];
    const start = m.index ?? 0;
    if (urls.some(([a, b]) => start >= a && start < b)) continue; // `https://host:3000/a.js` is a link, not a file
    // Trailing sentence punctuation is not part of the name.
    run = run.replace(/[.\-]+$/, '');
    let end = start + run.length;
    if (end === start) continue;
    const rest = text.slice(end);
    const suffix = COLON_SUFFIX.exec(rest) ?? PAREN_SUFFIX.exec(rest);
    // If anything was trimmed the suffix cannot follow it.
    const trimmed = start + m[0].length !== end;
    const hasLine = !!suffix && !trimmed;
    if (!believable(run, hasLine)) continue;
    let line: number | null = null;
    let col: number | null = null;
    if (suffix && !trimmed) {
      line = Number(suffix[1]);
      col = suffix[2] ? Number(suffix[2]) : null;
      end += suffix[0].length;
    }
    out.push({ start, end, path: run, line, col });
  }
  return out;
}
