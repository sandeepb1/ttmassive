// Icons are Lucide, drawn at 16px and coloured with currentColor. Call sites ask for them by our own
// names, so the set can change without touching them.

import {
  Bot, ChevronDown, ChevronRight, ChevronUp, Columns2, Command, createElement, Dot, FileCheck, FileText, Folder,
  GitBranch, Globe, Grid2x2, Image, Info, Layers, ListChecks, Menu, PanelLeft, PanelRight, Pencil, Plus, Route,
  Search, SlidersHorizontal, Sparkles, Terminal, X,
  type IconNode,
} from 'lucide';

export type IconName = 'shell' | 'read' | 'edit' | 'search' | 'web' | 'agent' | 'todo' | 'skill' | 'dot' | 'chevron' | 'plus' | 'panel-left' | 'panel-right' | 'sliders' | 'menu' | 'info' | 'files' | 'image' | 'close' | 'branch' | 'folder' | 'columns' | 'grid' | 'chevron-up' | 'chevron-down' | 'command' | 'memory' | 'route';

const NODES: Record<IconName, IconNode> = {
  shell: Terminal,
  read: FileText,
  edit: Pencil,
  search: Search,
  web: Globe,
  agent: Bot,
  todo: ListChecks,
  skill: Sparkles,
  dot: Dot,
  chevron: ChevronRight,
  'chevron-up': ChevronUp,
  'chevron-down': ChevronDown,
  plus: Plus,
  'panel-left': PanelLeft,
  'panel-right': PanelRight,
  sliders: SlidersHorizontal,
  menu: Menu,
  info: Info,
  files: FileCheck,
  image: Image,
  close: X,
  branch: GitBranch,
  folder: Folder,
  columns: Columns2,
  grid: Grid2x2,
  memory: Layers,
  route: Route,
  command: Command,
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
  return createElement(NODES[name], { width: 16, height: 16, 'aria-hidden': 'true', class: cls }) as SVGSVGElement;
}
