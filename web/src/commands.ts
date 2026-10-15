// The things the command bar can do. Each is wired to a function the app already has; this module
// only decides which apply right now (a pane with no agent has no "Change model") and how they read.

import type { Item, Step } from './command-model';
import { openOrch } from './orch/entry';
import type { Pane } from './pane';
import { getBellSound, getOptionMeta, setBellSound, setOptionMeta } from './prefs';
import type { PresetInfo } from './dirs';
import type { SessionView } from './sessions';

export type { PresetInfo } from './dirs';

/** What the command bar needs from the app. */
export interface Ctx {
  focused: () => Pane | null;
  panes: () => Pane[];
  canSplit: () => boolean;
  paneLimit: () => number;
  sessions: () => Promise<SessionView[]>;
  presets: () => Promise<PresetInfo[]>;
  focusPane: (p: Pane) => void;
  switchTo: (session: string) => void;
  /** Start a session (a shell when `preset` is undefined) in `dir` and show it in the focused pane. */
  startHere: (preset: string | undefined, dir: string) => Promise<void>;
  /** Open the new-session sheet, with `dir` (a project's folder) filled in when there is one. */
  newSession: (dir?: string) => void;
  showAnother: () => void;
  closePane: (p: Pane) => void;
  openLayout: () => void;
  openSettings: () => void;
  openFile: (session: string, path: string) => void;
}

const base = (path: string) => path.split('/').filter(Boolean).pop() ?? path;

export const presetLabel = (name: string) => (name === 'shell' ? 'Shell' : name[0]!.toUpperCase() + name.slice(1));

/** The command bar's first screen. */
export function rootStep(ctx: Ctx): Step {
  const built = buildItems(ctx);
  return { placeholder: 'Type a command, or a session name…', items: () => built };
}

async function buildItems(ctx: Ctx): Promise<Item[]> {
  const [sessions, presets] = await Promise.all([ctx.sessions().catch(() => []), ctx.presets().catch(() => [])]);
  const pane = ctx.focused();
  const here = pane ? sessions.find((s) => s.id === pane.name) : undefined;
  const project = here?.project ?? here?.cwd ?? null;
  const items: Item[] = [];
  const add = (it: Item) => items.push(it);
  const panes = ctx.panes();

  // ---- Start something
  add({ id: 'start:new', label: 'New session…', hint: 'Pick a folder, then Claude, Codex or a shell', group: 'Start', keywords: 'launch open folder directory project', run: () => ctx.newSession(project ?? undefined) });
  if (project) {
    for (const p of [{ name: 'shell' }, ...presets]) {
      add({
        id: `start:${p.name}`,
        label: `${presetLabel(p.name)} here`,
        hint: `new session in ${project}`,
        group: 'Start',
        keywords: `launch open new ${p.name} ${base(project)}`,
        run: () => void ctx.startHere(p.name === 'shell' ? undefined : p.name, project),
      });
    }
  }

  add({ id: 'start:orchestrator', label: 'Orchestrator mode…', hint: 'Plan work as a flow of Claude Code and Codex agents', group: 'Start', keywords: 'orchestrator agents flow multi agent workflow team research build judge', run: () => openOrch() });

  // ---- Panes
  if (ctx.canSplit() && panes.length < ctx.paneLimit()) {
    add({ id: 'pane:another', label: 'Show another session in a new pane', group: 'Panes', keywords: 'split tile', run: ctx.showAnother });
  }
  if (panes.length > 1 && pane) add({ id: 'pane:close', label: 'Close this pane', group: 'Panes', keywords: 'remove tile', run: () => ctx.closePane(pane) });
  if (ctx.canSplit()) add({ id: 'pane:layout', label: 'Pane layouts…', group: 'Panes', keywords: 'grid tiles arrange', run: ctx.openLayout });
  if (panes.length > 1) {
    panes.forEach((p, i) => {
      if (p === pane) return;
      add({ id: `pane:go:${p.name}`, label: `Go to pane ${i + 1}`, hint: p.name, group: 'Panes', keywords: `focus tile ${p.name}`, run: () => ctx.focusPane(p) });
    });
  }

  // ---- Agent (the focused pane's Claude or Codex)
  const a = pane?.actions;
  if (pane && a && pane.agent) {
    const who = pane.agent === 'codex' ? 'Codex' : 'Claude';
    add({ id: 'agent:model', label: 'Change model', hint: who, group: 'Agent', keywords: 'effort reasoning', run: a.model });
    add({ id: 'agent:mode', label: 'Change mode', hint: who, group: 'Agent', keywords: 'plan permissions auto', run: a.mode });
    add({ id: 'agent:more', label: 'Compact, clear and more…', hint: who, group: 'Agent', keywords: 'context new conversation', run: a.more });
    add({ id: 'agent:resume', label: 'Resume a conversation…', hint: who, group: 'Agent', keywords: 'history previous', run: a.resume });
  }

  // ---- Terminal
  if (pane) {
    if (pane.canChat) {
      const to = pane.view === 'chat' ? 'term' : 'chat';
      add({ id: 'view:toggle', label: `Show the ${to === 'term' ? 'terminal' : 'chat'}`, group: 'Terminal', keywords: 'view switch', run: () => pane.setView(to, true) });
    }
    if (pane.view === 'term') {
      add({ id: 'term:search', label: 'Search history', group: 'Terminal', keywords: 'find scrollback', run: () => pane.openSearch() });
      add({ id: 'term:prev', label: 'Previous message', group: 'Terminal', keywords: 'turn jump up', run: () => pane.stepTurns('older') });
      add({ id: 'term:next', label: 'Next message', group: 'Terminal', keywords: 'turn jump down', run: () => pane.stepTurns('newer') });
      add({ id: 'term:select', label: 'Select text', hint: 'Copy from the screen and history as text', group: 'Terminal', keywords: 'copy', run: () => pane.selectText() });
    }
    add({ id: 'file:open', label: 'Open file…', hint: 'By path, in the viewer', group: 'Terminal', keywords: 'read view', step: () => fileStep(ctx, pane.name) });
  }

  // ---- Settings
  add({ id: 'settings', label: 'Settings', group: 'App', keywords: 'theme text size notifications', run: ctx.openSettings });
  add({
    id: 'pref:meta',
    label: `Option key as Meta: ${getOptionMeta() ? 'on' : 'off'}`,
    hint: `Turn ${getOptionMeta() ? 'off' : 'on'}`,
    group: 'App',
    keywords: 'alt keyboard',
    run: () => setOptionMeta(!getOptionMeta()),
  });
  add({
    id: 'pref:bell',
    label: `Bell sound: ${getBellSound() ? 'on' : 'off'}`,
    hint: `Turn ${getBellSound() ? 'off' : 'on'}`,
    group: 'App',
    keywords: 'mute audio',
    run: () => setBellSound(!getBellSound()),
  });

  // ---- Sessions, last: they are many and have the search to themselves
  for (const s of sessions) {
    if (s.id === pane?.name) continue;
    add({
      id: `switch:${s.id}`,
      label: `Switch to ${s.id}`,
      hint: [s.cwd, s.branch, s.status !== 'running' ? s.status : s.activity].filter(Boolean).join(' · '),
      group: 'Sessions',
      keywords: `${s.preset ?? ''} ${s.agent ?? ''} ${s.note ?? ''} ${s.tags.join(' ')}`,
      run: () => ctx.switchTo(s.id),
    });
  }
  return items;
}

/** Second screen of "Open file…": the path you type is the one command. */
function fileStep(ctx: Ctx, session: string): Step {
  return {
    placeholder: 'A file path: relative to the session folder, or ~/…',
    filtered: true,
    items: (typed) => {
      const path = typed.trim();
      return path ? [{ id: 'file:open:go', label: `Open ${path}`, run: () => ctx.openFile(session, path) }] : [];
    },
  };
}
