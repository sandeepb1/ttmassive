// Icons are Lucide, drawn at 16px and coloured with currentColor. Call sites ask for them by our own
// names, so the set can change without touching them.

import {
  ArrowDown, ArrowLeft, ArrowUp, BookOpen, Bot, ChevronDown, ChevronRight, ChevronUp, Columns2, Command, createElement, Dot, Ellipsis, FileCheck, FileText, Folder,
  Gavel, GitBranch, Globe, Hammer, Grid2x2, Image, Info, Layers, ListChecks, Menu, Minus, PanelLeft, PanelRight, Pause, Pencil, Play, Plus, Radar, RotateCcw, Route,
  Save, Scale, Search, ShieldAlert, SlidersHorizontal, Sparkles, Square, Terminal, Trash2, WandSparkles, Workflow, X, Check, CircleAlert,
  type IconNode,
} from 'lucide';

export type IconName = 'shell' | 'read' | 'edit' | 'search' | 'web' | 'agent' | 'todo' | 'skill' | 'dot' | 'chevron' | 'plus' | 'panel-left' | 'panel-right' | 'sliders' | 'menu' | 'info' | 'files' | 'image' | 'close' | 'branch' | 'folder' | 'columns' | 'grid' | 'chevron-up' | 'chevron-down' | 'command' | 'memory' | 'route' | 'more' | 'arrow-up' | 'arrow-down' | 'stop' | 'minus' | 'shield-alert' | 'play' | 'pause' | 'check' | 'alert' | 'back' | 'save' | 'trash' | 'wand' | 'retry' | 'radar' | 'workflow' | 'book' | 'scale' | 'gavel' | 'hammer';

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
  more: Ellipsis,
  'arrow-up': ArrowUp,
  'arrow-down': ArrowDown,
  stop: Square,
  minus: Minus,
  'shield-alert': ShieldAlert,
  play: Play,
  pause: Pause,
  check: Check,
  alert: CircleAlert,
  back: ArrowLeft,
  save: Save,
  trash: Trash2,
  wand: WandSparkles,
  retry: RotateCcw,
  radar: Radar,
  workflow: Workflow,
  book: BookOpen,
  scale: Scale,
  gavel: Gavel,
  hammer: Hammer,
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
