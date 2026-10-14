// Small line icons, drawn on a 16px grid and coloured with currentColor.

const NS = 'http://www.w3.org/2000/svg';

export type IconName = 'shell' | 'read' | 'edit' | 'search' | 'web' | 'agent' | 'todo' | 'skill' | 'dot' | 'chevron' | 'plus' | 'panel-left' | 'panel-right' | 'sliders' | 'menu' | 'info' | 'files' | 'image' | 'close' | 'branch' | 'folder' | 'columns' | 'grid';

/** Each icon is a list of SVG path `d` strings, plus circles as [cx, cy, r]. */
const SHAPES: Record<IconName, { d: string[]; circles?: [number, number, number][] }> = {
  shell: { d: ['M3 4.5l3.5 3.5L3 11.5', 'M8.5 12H13'] },
  read: { d: ['M4 2h5l3 3v9H4z', 'M9 2v3h3', 'M6 8.5h4M6 11h4'] },
  edit: { d: ['M3 13l.8-3.2L10.5 3.1l2.4 2.4-6.7 6.7z', 'M9.2 4.4l2.4 2.4'] },
  search: { d: ['M10 10l3.5 3.5'], circles: [[7, 7, 4.2]] },
  web: { d: ['M2 8h12', 'M8 2c2.2 2.2 2.2 9.8 0 12', 'M8 2c-2.2 2.2-2.2 9.8 0 12'], circles: [[8, 8, 6]] },
  agent: { d: ['M8 2l5 3v6l-5 3-5-3V5z', 'M8 7v2'] },
  todo: { d: ['M2.8 4.2l1.3 1.3L6.3 3.2', 'M2.8 10.2l1.3 1.3 2.2-2.3', 'M8.5 4.8H13.5M8.5 10.8H13.5'] },
  skill: { d: ['M8 2l1.4 4.6L14 8l-4.6 1.4L8 14l-1.4-4.6L2 8l4.6-1.4z'] },
  dot: { d: [], circles: [[8, 8, 1.6]] },
  chevron: { d: ['M6 4l4 4-4 4'] },
  plus: { d: ['M8 3v10M3 8h10'] },
  'panel-left': { d: ['M2.5 3.5h11v9h-11z', 'M6 3.5v9'] },
  'panel-right': { d: ['M2.5 3.5h11v9h-11z', 'M10 3.5v9'] },
  sliders: { d: ['M2.5 5h11M2.5 11h11'], circles: [[6, 5, 1.7], [10, 11, 1.7]] },
  menu: { d: ['M3 4.5h10M3 8h10M3 11.5h10'] },
  info: { d: ['M8 7.4v3.8'], circles: [[8, 8, 6], [8, 5.1, 0.4]] },
  files: { d: ['M4 2.5h5l3 3v8H4z', 'M9 2.5v3h3', 'M6.2 9l1.3 1.3L9.8 8'] },
  image: { d: ['M2.5 3.5h11v9h-11z', 'M2.5 11.2l3.4-3.4 3 3 2-2 2.6 2.6'], circles: [[10.5, 6.2, 1]] },
  close: { d: ['M4 4l8 8M12 4l-8 8'] },
  branch: { d: ['M5 3v7.2', 'M11 5.8c0 3-6 1.6-6 4.4'], circles: [[5, 2.6, 1.4], [11, 4.6, 1.4], [5, 12.4, 1.4]] },
  columns: { d: ['M2.5 3.5h11v9h-11z', 'M8 3.5v9'] },
  grid: { d: ['M2.5 3.5h11v9h-11z', 'M8 3.5v9', 'M2.5 8h11'] },
  folder: { d: ['M2.5 4.5h3.8l1.4 1.5h5.8v6.5h-11z'] },
};

/** The category a tool belongs to, for its icon and tint. */
export type ToolCategory = 'shell' | 'read' | 'edit' | 'search' | 'web' | 'agent' | 'todo' | 'skill' | 'other';

const TOOL_CATEGORY: Record<string, ToolCategory> = {
  Bash: 'shell',
  Read: 'read',
  Edit: 'edit',
  MultiEdit: 'edit',
  Write: 'edit',
  NotebookEdit: 'edit',
  Glob: 'search',
  Grep: 'search',
  WebSearch: 'search',
  WebFetch: 'web',
  Task: 'agent',
  Agent: 'agent',
  Skill: 'skill',
  TodoWrite: 'todo',
};

export function toolCategory(tool: string): ToolCategory {
  return TOOL_CATEGORY[tool] ?? 'other';
}

export function toolIconName(cat: ToolCategory): IconName {
  return cat === 'other' ? 'dot' : cat;
}

export function icon(name: IconName, cls = 'icon-svg'): SVGSVGElement {
  const svg = document.createElementNS(NS, 'svg');
  svg.setAttribute('viewBox', '0 0 16 16');
  svg.setAttribute('width', '16');
  svg.setAttribute('height', '16');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '1.5');
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', cls);
  const shape = SHAPES[name];
  for (const d of shape.d) {
    const p = document.createElementNS(NS, 'path');
    p.setAttribute('d', d);
    svg.append(p);
  }
  for (const [cx, cy, r] of shape.circles ?? []) {
    const c = document.createElementNS(NS, 'circle');
    c.setAttribute('cx', String(cx));
    c.setAttribute('cy', String(cy));
    c.setAttribute('r', String(r));
    if (name === 'dot') c.setAttribute('fill', 'currentColor');
    svg.append(c);
  }
  return svg;
}
