// What `/api/sessions/{id}/chat` returns, and the pure logic for keeping a local copy of it in
// step. No DOM here, so node can test it.

import type { Analysis } from './protocol';

export type ToolStatus = 'running' | 'ok' | 'error';
export type QuestionState = 'pending' | 'answered' | 'cancelled';

export interface QuestionOption {
  label: string;
  description: string;
}

export interface Question {
  header: string;
  question: string;
  multi_select: boolean;
  options: QuestionOption[];
}

export interface Answer {
  question: string;
  answer: string;
}

export interface EditPair {
  old: string;
  new: string;
}

export interface DiffData {
  path: string;
  edits: EditPair[];
}

export type ChatBody =
  | { kind: 'user'; text: string; images: number }
  | { kind: 'assistant'; text: string }
  | { kind: 'thinking'; text: string }
  | { kind: 'command'; text: string }
  | { kind: 'notice'; text: string }
  | {
      kind: 'tool';
      tool: string;
      summary: string;
      input: string | null;
      output: string | null;
      status: ToolStatus;
      diff: DiffData | null;
    }
  | { kind: 'question'; questions: Question[]; state: QuestionState; answers: Answer[] };

export type ChatItem = { id: string; seq: number; ts: number | null } & ChatBody;

export type ModeId = 'default' | 'auto' | 'manual' | 'accept_edits' | 'plan' | 'bypass';

export interface ModeInfo {
  id: ModeId;
  label: string;
  hint: string;
}

export interface ChatResponse {
  epoch: number;
  seq: number;
  reset: boolean;
  items: ChatItem[];
  model: string | null;
  effort: string | null;
  /** A model name from a `/model` confirmation that no reply has reported yet. */
  model_label: string | null;
  eligible: boolean;
  agent: 'claude' | 'codex' | null;
  ready: boolean;
  screen: Analysis | null;
  /** The agent's plan/build mode, read from its footer. */
  mode: ModeId | null;
  /** The modes this session can be switched between. */
  modes: ModeInfo[];
  /** Started with permission checks skipped. */
  yolo: boolean;
  /** The reasoning level on the agent's screen right now (newer than `effort`). */
  effort_live: string | null;
  /** Claude's greyed-out suggested reply in its empty input box. */
  suggestion: string | null;
}

/** The client's copy of one conversation. */
export interface ChatStore {
  epoch: number | null;
  seq: number;
  items: ChatItem[];
  index: Map<string, number>;
  model: string | null;
  effort: string | null;
  modelLabel: string | null;
  mode: ModeId | null;
  modes: ModeInfo[];
  yolo: boolean;
  effortLive: string | null;
  suggestion: string | null;
  /** The last mode that was not plan: where "Build" goes back to. */
  lastBuild: ModeId | null;
  /**
   * A change you just made. The log only reports the model and effort with the agent's next
   * reply, so until then the chip shows your choice. `from` is what the log said when you chose;
   * once the log says something else it has caught up and the override is dropped.
   */
  modelOverride: { from: string | null; label: string } | null;
  effortOverride: { from: string | null; level: string } | null;
}

export function newStore(): ChatStore {
  return {
    epoch: null,
    seq: 0,
    items: [],
    index: new Map(),
    model: null,
    effort: null,
    modelLabel: null,
    mode: null,
    modes: [],
    yolo: false,
    effortLive: null,
    suggestion: null,
    lastBuild: null,
    modelOverride: null,
    effortOverride: null,
  };
}

/** Remember a model or effort change made through the picker. */
export function setOverride(s: ChatStore, change: { model?: string; effort?: string }) {
  if (change.model) s.modelOverride = { from: s.model, label: change.model };
  if (change.effort) s.effortOverride = { from: s.effort, level: change.effort };
}

/**
 * Fold a response into the store. Returns the ids that are new or changed (in log order) and
 * whether the whole list was replaced, in which case the view should redraw everything.
 */
export function applyChat(s: ChatStore, r: ChatResponse): { reset: boolean; touched: string[] } {
  const reset = r.reset || s.epoch !== r.epoch;
  if (reset) {
    s.items = [];
    s.index = new Map();
  }
  const touched: string[] = [];
  for (const it of r.items) {
    const pos = s.index.get(it.id);
    if (pos === undefined) {
      s.index.set(it.id, s.items.length);
      s.items.push(it);
    } else {
      s.items[pos] = it;
    }
    touched.push(it.id);
  }
  s.epoch = r.epoch;
  s.seq = r.seq;
  s.model = r.model ?? s.model;
  s.effort = r.effort ?? s.effort;
  s.modelLabel = r.model_label;
  // Switching passes through the modes in between (Auto to Plan goes via Manual and Accept
  // edits), so a mode counts as "where you were building" only once it is seen twice running.
  const steady = r.mode !== null && r.mode === s.mode;
  s.mode = r.mode;
  s.modes = r.modes;
  s.yolo = r.yolo;
  s.effortLive = r.effort_live;
  s.suggestion = r.suggestion ?? null;
  if (steady && r.mode !== 'plan') s.lastBuild = r.mode;
  if (s.modelOverride && s.model !== s.modelOverride.from) s.modelOverride = null;
  if (s.effortOverride && s.effort !== s.effortOverride.from) s.effortOverride = null;
  return { reset, touched };
}

/** The question the agent is waiting on, if any (the newest one still pending). */
export function pendingQuestion(items: ChatItem[]): (ChatItem & { kind: 'question' }) | undefined {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it.kind === 'question' && it.state === 'pending') return it;
  }
  return undefined;
}

const cap = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s);

/** `claude-sonnet-5-5` → `Sonnet 5.5`, `gpt-6.1-sol` → `GPT-6.1-Sol`; anything else as it is. */
export function prettyModel(id: string | null): string {
  if (!id) return '';
  const claude = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$/.exec(id);
  if (claude) return `${cap(claude[1])} ${claude[2]}${claude[3] ? `.${claude[3]}` : ''}`;
  if (/^gpt-/i.test(id)) return id.split('-').map((p, i) => (i === 0 ? p.toUpperCase() : cap(p))).join('-');
  return id;
}

/** What the chip shows now, counting a change that the log has not reported yet. */
export function chipFor(s: ChatStore): string {
  const model = s.modelOverride ? s.modelOverride.label : (s.modelLabel ?? prettyModel(s.model));
  return [model, currentEffort(s)].filter(Boolean).join(' · ');
}

/** The reasoning level now: your change if the screen has not caught up, else the screen, else the log. */
export function currentEffort(s: ChatStore): string | null {
  return s.effortOverride?.level ?? s.effortLive ?? s.effort;
}

/** The text of the model chip: `Sonnet 5.5 · medium`. */
export function modelChip(model: string | null, effort: string | null): string {
  return [prettyModel(model), effort].filter(Boolean).join(' · ');
}

/** Which view a session opens in. A saved choice wins; otherwise chat when it is possible. */
export function chooseView(saved: string | null, eligible: boolean): 'chat' | 'term' {
  if (!eligible) return 'term';
  return saved === 'term' ? 'term' : 'chat';
}

/** Was a message we just sent already recorded in the log? Compared as trimmed text. */
export function isRecorded(items: ChatItem[], text: string, sinceSeq: number): boolean {
  const want = text.trim();
  return items.some((i) => i.kind === 'user' && i.seq > sinceSeq && i.text.trim() === want);
}

/** A Codex level as the log and the chip name it: `Extra high` is `xhigh`, the rest lower-case. */
export function levelKey(label: string): string {
  const l = label.trim().toLowerCase();
  return l === 'extra high' ? 'xhigh' : l;
}

/** You are about to leave a build mode for plan: that is the mode "Build" should come back to. */
export function noteLeavingBuild(s: ChatStore, mode: ModeId | null) {
  if (mode && mode !== 'plan') s.lastBuild = mode;
}

/** What "Build" means now: the mode you were in before plan, or a sensible default. */
export function buildTarget(s: ChatStore): ModeInfo | null {
  const build = s.modes.filter((m) => m.id !== 'plan');
  if (build.length === 0) return null;
  const wanted = s.lastBuild ?? (s.modes.some((m) => m.id === 'accept_edits') ? 'accept_edits' : 'default');
  return build.find((m) => m.id === wanted) ?? build[0];
}

/** The text of the mode chip: `Plan`, `Accept edits`, with a bolt when permissions are skipped. */
export function modeChipText(s: ChatStore): string {
  if (!s.mode) return '';
  const label = s.modes.find((m) => m.id === s.mode)?.label ?? s.mode;
  return s.yolo ? `⚡ ${label}` : label;
}

const EFFORT_ORDER = ['low', 'medium', 'high', 'xhigh', 'max'];

/** One notch up or down from `current`, or null at either end or for a level not in the list. */
export function stepEffort(current: string | null, up: boolean): string | null {
  const i = current ? EFFORT_ORDER.indexOf(current) : -1;
  if (i === -1) return null;
  return EFFORT_ORDER[i + (up ? 1 : -1)] ?? null;
}

/** The most recent message you sent, for editing and sending again. */
export function lastUserMessage(items: ChatItem[]): string | null {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it.kind === 'user' && it.text.trim() !== '') return it.text;
  }
  return null;
}
