import './proto.css';
import { THEMES, mix, onAccent, type Theme } from '../src/theme';
import { createElement, Terminal, FileText, Pencil, ChevronDown, ArrowUp, Plus, Paperclip, Check, X, type IconNode } from 'lucide';
import { icon as customIcon, type IconName } from '../src/icons';

type Dir = 'a' | 'b' | 'c' | 'd';
const DIRS: { id: Dir; name: string; blurb: string }[] = [
  { id: 'a', name: 'A · Current, normalized', blurb: '10/14px radii, 40px controls' },
  { id: 'b', name: 'B · shadcn neutral', blurb: '6/8px radii, hairlines, overlay shadow' },
  { id: 'c', name: 'C · Terminal-native dense', blurb: '3px radii, 30px controls, mono-forward' },
  { id: 'd', name: 'D · Soft / iOS', blurb: '16/24px radii, borderless, 48px targets' },
];

/** theme.ts palette -> shadcn semantic vars (the derivation step 1 will move into theme.ts). */
function vars(t: Theme, dir: Dir): Record<string, string> {
  const dark = t.scheme === 'dark';
  const lift = dark ? '#ffffff' : '#000000';
  const card = mix(t.bg, lift, dir === 'd' ? 7 : 4);
  const strongBorder = dir === 'd' ? 0 : dir === 'c' ? 16 : dir === 'b' ? 10 : 14;
  return {
    '--background': t.bg,
    '--foreground': t.fg,
    '--card': card,
    '--muted': mix(t.bg, lift, 9),
    '--muted-foreground': mix(t.fg, t.bg, 42),
    '--secondary': mix(t.bg, lift, 12),
    '--primary': t.claude,
    '--primary-foreground': onAccent(t),
    '--border': mix(t.bg, lift, strongBorder),
    '--ring': t.accent,
    '--ok': t.ok,
    '--warn': t.warn,
    '--destructive': t.danger,
  };
}

let themeId = THEMES[0]!.id;
let lucideOn = true;

const lucide = (node: IconNode, cls = 'size-4') => createElement(node, { class: cls + ' shrink-0', 'stroke-width': 1.75 }) as unknown as SVGElement;
const custom = (n: IconName) => customIcon(n);

function h<K extends keyof HTMLElementTagNameMap>(tag: K, cls = '', ...kids: (Node | string)[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  e.className = cls;
  for (const k of kids) e.append(k);
  return e;
}

function phone(dir: Dir): HTMLElement {
  const I = {
    shell: () => (lucideOn ? lucide(Terminal) : custom('shell')),
    read: () => (lucideOn ? lucide(FileText) : custom('read')),
    edit: () => (lucideOn ? lucide(Pencil) : custom('edit')),
    chev: () => (lucideOn ? lucide(ChevronDown, 'size-3.5') : custom('chevron')),
    plus: () => (lucideOn ? lucide(Plus) : custom('plus')),
    send: () => lucide(ArrowUp),
    clip: () => lucide(Paperclip),
    check: () => lucide(Check, 'size-3.5'),
    x: () => lucide(X, 'size-3.5'),
  };
  const dense = dir === 'c';
  const mono = dense ? 'font-mono' : '';

  const btn = (label: string | Node, kind: 'primary' | 'secondary' | 'ghost' | 'outline' | 'destructive', extra = '') => {
    const kinds = {
      primary: 'bg-primary text-primary-foreground hover:opacity-90',
      secondary: 'bg-secondary text-foreground hover:bg-muted',
      ghost: 'text-muted-foreground hover:bg-muted hover:text-foreground',
      outline: 'ui-border bg-transparent text-foreground hover:bg-muted',
      destructive: 'bg-destructive text-primary-foreground hover:opacity-90',
    };
    const b = h('button', `inline-flex h-(--ctl) items-center justify-center gap-1.5 rounded-ui px-3.5 text-(length:--fs) font-medium transition focus-visible:outline-2 focus-visible:outline-ring ${kinds[kind]} ${extra}`);
    b.append(label);
    return b;
  };
  const chip = (label: string, tone = 'text-muted-foreground') =>
    h('span', `inline-flex h-6 items-center gap-1 rounded-full bg-muted px-2.5 text-xs ${tone} ${mono}`, label);

  const tool = (ic: SVGElement | HTMLElement, name: string, arg: string, state: 'ok' | 'run') => {
    const row = h('div', `ui-border flex min-h-(--ctl) items-center gap-2 rounded-ui bg-card px-3 text-(length:--fs) ${mono}`);
    ic.classList.add('shrink-0', 'text-muted-foreground');
    row.append(ic, h('span', 'font-medium', name), h('span', 'min-w-0 flex-1 truncate text-muted-foreground', arg));
    if (state === 'ok') {
      const ok = I.check();
      ok.setAttribute('class', 'size-3.5 text-ok');
      row.append(ok);
    } else row.append(h('span', 'size-2 animate-pulse rounded-full bg-primary'));
    return row;
  };

  const frame = h('div', 'flex h-[760px] w-[390px] shrink-0 flex-col overflow-hidden rounded-ui-lg bg-background');
  frame.dataset.dir = dir;
  frame.style.border = '1px solid var(--border)';

  // Header
  const head = h('div', 'flex items-center gap-2 border-b border-border px-3 py-2');
  head.append(
    h('span', 'size-2 rounded-full bg-primary'),
    h('div', 'min-w-0 flex-1', h('div', `truncate text-(length:--fs) font-semibold ${mono}`, 'ttmassive'), h('div', 'truncate text-xs text-muted-foreground', 'claude · opus 5.5 · main')),
    chip('42% ctx'),
    btn(I.plus(), 'ghost', 'w-(--ctl) px-0'),
  );

  // Stream
  const stream = h('div', 'flex min-h-0 flex-1 flex-col gap-(--gap) overflow-y-auto p-3');
  const user = h('div', `ml-auto max-w-[85%] rounded-ui-lg bg-secondary px-3.5 py-2.5 text-(length:--fs) ${mono}`, 'Add a Cloudflare tunnel option and reinstall the service.');
  const asst = h('div', `max-w-[95%] text-(length:--fs) leading-relaxed ${mono}`);
  asst.append(
    h('p', '', 'I added a '),
  );
  (asst.firstChild as HTMLElement).append(h('code', 'rounded-ui bg-muted px-1.5 py-0.5 font-mono text-[0.85em]', 'cloudflare'), ' provider that runs ', h('code', 'rounded-ui bg-muted px-1.5 py-0.5 font-mono text-[0.85em]', 'cloudflared'), ' with the token passed via the environment.');
  const code = h('pre', 'ui-border mt-2 overflow-x-auto rounded-ui bg-card p-3 font-mono text-xs leading-5', 'tunnel_token = "…"\ntunnel_domain = "ttm.nvy.me"');
  asst.append(code);

  const perm = h('div', 'ui-border flex flex-col gap-3 rounded-ui-lg bg-card p-3');
  perm.style.boxShadow = 'var(--sh)';
  perm.append(
    h('div', `text-(length:--fs) ${mono}`, h('span', 'font-medium', 'Allow '), h('span', 'text-warn', 'launchctl kickstart -k'), '?'),
    h('div', 'flex gap-2', btn('Allow once', 'primary', 'flex-1'), btn('Always', 'secondary', 'flex-1'), btn(I.x(), 'outline', 'w-(--ctl) px-0')),
  );

  stream.append(
    user,
    asst,
    tool(I.read(), 'Read', 'src/tunnel.rs', 'ok'),
    tool(I.edit(), 'Edit', 'src/config.rs', 'ok'),
    tool(I.shell(), 'Bash', 'cargo test --release', 'run'),
    perm,
  );

  // Composer
  const area = h('textarea', `w-full resize-none bg-transparent px-1 text-(length:--fs) outline-none placeholder:text-muted-foreground ${mono}`);
  area.rows = 2;
  area.placeholder = 'Message Claude…';
  const send = btn(I.send(), 'primary', 'w-(--ctl) px-0');
  const comp = h('div', 'border-t border-border p-3');
  const box = h('div', 'ui-border flex flex-col gap-2 rounded-ui-lg bg-card p-2 focus-within:border-ring');
  box.append(
    area,
    h('div', 'flex items-center gap-1.5', btn(I.clip(), 'ghost', 'w-(--ctl) px-0'), btn(h('span', 'flex items-center gap-1.5', 'Opus 5.5', I.chev()), 'ghost', 'px-2'), chip('plan'), h('span', 'flex-1'), send),
  );
  comp.append(box);

  frame.append(head, stream, comp);
  return frame;
}

function paint() {
  const t = THEMES.find((x) => x.id === themeId)!;
  const stage = document.getElementById('stage')!;
  stage.replaceChildren();
  stage.className = 'flex gap-6 overflow-x-auto p-6';
  for (const d of DIRS) {
    const col = h('div', 'flex flex-col gap-2');
    col.dataset.dir = d.id;
    for (const [k, v] of Object.entries(vars(t, d.id))) col.style.setProperty(k, v);
    col.append(h('div', 'px-1', h('div', 'text-sm font-semibold', d.name), h('div', 'text-xs opacity-60', d.blurb)), phone(d.id));
    stage.append(col);
  }
  document.body.style.setProperty('--background', t.bg);
  document.body.style.setProperty('--foreground', t.fg);
}

const bar = document.getElementById('bar')!;
bar.className = 'sticky left-0 top-0 z-10 flex flex-wrap items-center gap-3 border-b border-white/10 bg-black/60 p-3 text-sm text-white backdrop-blur';
const sel = h('select', 'rounded bg-white/10 px-2 py-1');
for (const t of THEMES) sel.append(new Option(t.name, t.id));
sel.onchange = () => { themeId = sel.value; paint(); };
const tog = h('label', 'flex items-center gap-1.5');
const cb = h('input') as HTMLInputElement;
cb.type = 'checkbox';
cb.checked = true;
cb.onchange = () => { lucideOn = cb.checked; paint(); };
tog.append(cb, 'Lucide icons (off = current custom set)');
bar.append(h('span', 'font-semibold', 'Theme'), sel, tog);
paint();
