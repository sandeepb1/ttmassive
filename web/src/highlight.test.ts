import assert from 'node:assert/strict';
import { test } from 'node:test';
import { highlight, languageFor, lineHtml, type Token } from './highlight.ts';

const kinds = (tokens: Token[]) => tokens.filter((t) => t.kind !== 'plain').map((t) => `${t.kind}:${t.text}`);
const lines = (name: string, src: string) => highlight(src, languageFor(name));

test('languages are picked by extension or name', () => {
  assert.ok(languageFor('main.rs'));
  assert.ok(languageFor('App.TSX'));
  assert.ok(languageFor('Makefile'));
  assert.equal(languageFor('notes.xyz'), null);
  assert.equal(languageFor('README'), null);
});

test('rust: keywords, strings, comments and numbers', () => {
  const [l] = lines('a.rs', 'let x = "hi"; // note');
  assert.deepEqual(kinds(l!), ['kw:let', 'str:"hi"', 'com:// note']);
  assert.deepEqual(kinds(lines('a.rs', 'fn f() -> u32 { 0x1F + 42 }')[0]!), ['kw:fn', 'num:0x1F', 'num:42']);
});

test('rust lifetimes are not strings', () => {
  const [l] = lines('a.rs', "fn f<'a>(x: &'a str) -> char { 'z' }");
  assert.deepEqual(kinds(l!), ['kw:fn', "str:'z'"]);
});

test('a block comment carries across lines', () => {
  const got = lines('a.ts', 'a /* start\nmiddle\nend */ b').map(kinds);
  assert.deepEqual(got, [['com:/* start'], ['com:middle'], ['com:end */']]);
});

test('strings honour escapes and stop at the end of the line', () => {
  assert.deepEqual(kinds(lines('a.js', 'x = "a\\"b" + 1')[0]!), ['str:"a\\"b"', 'num:1']);
  assert.deepEqual(kinds(lines('a.js', 'x = "never closed')[0]!), ['str:"never closed']);
});

test('python: hash comments and triple-quoted strings across lines', () => {
  const got = lines('a.py', 'def f():  # hi\n    """doc\n    more"""\n    return 1').map(kinds);
  assert.deepEqual(got, [['kw:def', 'com:# hi'], ['str:"""doc'], ['str:    more"""'], ['kw:return', 'num:1']]);
});

test('a hash inside a string is not a comment', () => {
  assert.deepEqual(kinds(lines('a.sh', 'echo "a # b" # real')[0]!), ['str:"a # b"', 'com:# real']);
});

test('json keys, values and literals', () => {
  assert.deepEqual(kinds(lines('a.json', '{"a": [1, true, null]}')[0]!), ['str:"a"', 'num:1', 'kw:true', 'kw:null']);
});

test('numbers inside identifiers are left alone', () => {
  assert.deepEqual(kinds(lines('a.rs', 'let v2 = x1 + 7')[0]!), ['kw:let', 'num:7']);
});

test('unknown files are plain, line by line', () => {
  assert.deepEqual(lines('x.bin', 'a\nb\n'), [[{ kind: 'plain', text: 'a' }], [{ kind: 'plain', text: 'b' }]]);
});

test('the newline that ends a file does not make an extra line', () => {
  assert.equal(lines('a.rs', 'a\nb\n').length, 2);
  assert.equal(lines('a.rs', 'a\nb').length, 2);
  assert.equal(lines('a.rs', '').length, 0);
  assert.equal(lines('a.rs', '\n\n').length, 2); // two blank lines
});

test('a huge line is left plain instead of being tokenized', () => {
  const [l] = lines('a.js', 'a = "x"'.repeat(1000));
  assert.deepEqual(l!.map((t) => t.kind), ['plain']);
});

test('html is escaped, including angle brackets inside strings and comments', () => {
  const html = lineHtml(lines('a.html.js', 'x = "<script>&</script>" // <b>')[0]!);
  assert.ok(!html.includes('<script>') && !html.includes('<b>'));
  assert.ok(html.includes('&lt;script&gt;&amp;&lt;/script&gt;'));
  assert.equal(lineHtml([{ kind: 'plain', text: 'a<b' }]), 'a&lt;b');
  assert.equal(lineHtml([{ kind: 'kw', text: 'if' }]), '<span class="tk-kw">if</span>');
});
