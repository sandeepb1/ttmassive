// A small syntax colourer for the file viewer: strings, comments, numbers and keywords for the
// languages agents work in. It is a tokenizer, not a parser, and never needs to be right about
// everything: a wrong colour is cosmetic. It is deliberately dependency-free and line-based.

export type Kind = 'kw' | 'str' | 'com' | 'num' | 'plain';
export interface Token {
  kind: Kind;
  text: string;
}

type Family = 'c' | 'hash' | 'json' | 'css' | 'plain';

interface Lang {
  family: Family;
  keywords: Set<string>;
  /** Python-style triple-quoted strings that run over several lines. */
  triple?: boolean;
}

const words = (s: string) => new Set(s.split(' '));

const KW_RUST = words(
  'as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while',
);
const KW_JS = words(
  'abstract async await break case catch class const continue debugger default delete do else enum export extends false finally for from function if implements import in instanceof interface let new null of private protected public readonly return static super switch this throw true try type typeof undefined var void while yield',
);
const KW_PY = words(
  'False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield self',
);
const KW_GO = words(
  'break case chan const continue default defer else fallthrough false for func go goto if import interface map nil package range return select struct switch true type var',
);
const KW_SH = words('if then else elif fi for while until do done case esac in function select return exit export local readonly set unset true false');
const KW_C = words(
  'auto break case char class const continue default do double else enum extern false float for goto if inline int long namespace new nullptr private protected public register return short signed sizeof static struct switch template this throw true try typedef union unsigned using virtual void volatile while',
);
const KW_JAVA = words(
  'abstract assert boolean break byte case catch char class const continue default do double else enum extends false final finally float for if implements import instanceof int interface long new null package private protected public return short static super switch this throw throws true try void volatile while fun val var when object override data sealed',
);
const KW_RUBY = words('alias and begin break case class def defined do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true undef unless until when while yield');
const KW_JSON = words('true false null');
const KW_TOML = words('true false');
const NONE = new Set<string>();

const LANGS: Record<string, Lang> = {};
const add = (exts: string, lang: Lang) => {
  for (const e of exts.split(' ')) LANGS[e] = lang;
};
add('rs', { family: 'c', keywords: KW_RUST });
add('ts tsx js jsx mjs cjs vue svelte', { family: 'c', keywords: KW_JS });
add('go', { family: 'c', keywords: KW_GO });
add('c h cc cpp hpp cs swift m mm dart zig', { family: 'c', keywords: KW_C });
add('java kt kts', { family: 'c', keywords: KW_JAVA });
add('py', { family: 'hash', keywords: KW_PY, triple: true });
add('sh bash zsh fish', { family: 'hash', keywords: KW_SH });
add('rb', { family: 'hash', keywords: KW_RUBY });
add('toml', { family: 'hash', keywords: KW_TOML, triple: true });
add('yaml yml conf ini cfg env', { family: 'hash', keywords: KW_TOML });
add('json jsonc', { family: 'json', keywords: KW_JSON });
add('css scss', { family: 'css', keywords: NONE });

const BY_NAME: Record<string, Lang> = {
  Makefile: { family: 'hash', keywords: NONE },
  Dockerfile: { family: 'hash', keywords: NONE },
};

/** The colouring rules for a file, from its name. Unknown files get none. */
export function languageFor(name: string): Lang | null {
  const ext = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1).toLowerCase() : '';
  return BY_NAME[name] ?? LANGS[ext] ?? null;
}

/** Lines past this are drawn plain: a minified file is one enormous line. */
const MAX_COLOURED_LINE = 2000;

const NUMBER = /^(?:0x[0-9a-fA-F_]+|0b[01_]+|\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?)/;
const IDENT = /^[A-Za-z_$][\w$]*/;
const CHAR_LITERAL = /^'(?:\\.[^']{0,8}|[^'\\])'/;

interface State {
  /** Inside a block comment that has not ended. */
  block: boolean;
  /** Inside a triple-quoted string, and which quote. */
  triple: string | null;
}

/** Colour one line, given where the previous one left off. */
function line(text: string, lang: Lang, st: State): Token[] {
  const out: Token[] = [];
  const push = (kind: Kind, t: string) => {
    if (!t) return;
    const last = out[out.length - 1];
    if (last && last.kind === kind) last.text += t;
    else out.push({ kind, text: t });
  };
  let i = 0;
  const n = text.length;
  const { family } = lang;
  while (i < n) {
    const rest = text.slice(i);
    if (st.block) {
      const end = text.indexOf('*/', i);
      if (end < 0) {
        push('com', rest);
        return out;
      }
      push('com', text.slice(i, end + 2));
      i = end + 2;
      st.block = false;
      continue;
    }
    if (st.triple) {
      const end = text.indexOf(st.triple, i);
      if (end < 0) {
        push('str', rest);
        return out;
      }
      push('str', text.slice(i, end + 3));
      i = end + 3;
      st.triple = null;
      continue;
    }
    const c = text[i]!;
    if (family === 'c' && rest.startsWith('//')) {
      push('com', rest);
      return out;
    }
    if ((family === 'c' || family === 'css') && rest.startsWith('/*')) {
      st.block = true;
      push('com', '/*');
      i += 2;
      continue;
    }
    if (family === 'hash' && c === '#') {
      push('com', rest);
      return out;
    }
    if (lang.triple && (rest.startsWith('"""') || rest.startsWith("'''"))) {
      st.triple = rest.slice(0, 3);
      push('str', st.triple);
      i += 3;
      continue;
    }
    if (c === '"' || c === '`' || c === "'") {
      if (c === "'" && family === 'c') {
        // A char literal, not a Rust lifetime (`'a`) or an apostrophe.
        const lit = CHAR_LITERAL.exec(rest);
        if (!lit) {
          push('plain', c);
          i++;
          continue;
        }
        push('str', lit[0]);
        i += lit[0].length;
        continue;
      }
      let j = i + 1;
      while (j < n && text[j] !== c) j += text[j] === '\\' ? 2 : 1;
      const end = Math.min(n, j + 1);
      push('str', text.slice(i, end));
      i = end;
      continue;
    }
    const num = /[\d]/.test(c) ? NUMBER.exec(rest) : null;
    if (num && !/[\w$]/.test(text[i - 1] ?? '')) {
      push('num', num[0]);
      i += num[0].length;
      continue;
    }
    const id = IDENT.exec(rest);
    if (id) {
      push(family !== 'css' && lang.keywords.has(id[0]) ? 'kw' : 'plain', id[0]);
      i += id[0].length;
      continue;
    }
    push('plain', c);
    i++;
  }
  return out;
}

/** Colour a whole file, one token list per line. Unknown languages come back as plain text. */
export function highlight(text: string, lang: Lang | null): Token[][] {
  const lines = text.split('\n');
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop(); // the newline that ends the file
  if (!lang) return lines.map((l) => [{ kind: 'plain', text: l }]);
  const st: State = { block: false, triple: null };
  return lines.map((l) => (l.length > MAX_COLOURED_LINE ? [{ kind: 'plain', text: l }] : line(l, lang, st)));
}

const ESC: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;' };

/** A line as HTML. Everything from the file is escaped, so this is safe to assign to innerHTML. */
export function lineHtml(tokens: Token[]): string {
  return tokens
    .map((t) => {
      const text = t.text.replace(/[&<>]/g, (ch) => ESC[ch]!);
      return t.kind === 'plain' ? text : `<span class="tk-${t.kind}">${text}</span>`;
    })
    .join('');
}
