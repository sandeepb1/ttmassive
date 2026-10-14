// What the page asks the server about folders, presets and starting sessions. Shared by the
// new-session sheet, the sidebar and the command bar, so there is one place that talks to them.

export interface PresetInfo {
  name: string;
  command?: string;
  /** Only Claude and Codex presets can skip permission checks. */
  agent?: 'claude' | 'codex' | 'other';
  /** Always started with permissions skipped (set in the config). */
  yolo?: boolean;
}

export interface GroupInfo {
  name: string;
  presets: string[];
}

/** A folder you have been working in, from `/api/dirs`. */
export interface RecentDir {
  path: string;
  name: string;
  short: string;
}

export interface BrowseResult {
  /** The folder whose subfolders are listed, with the home folder as `~`. Empty if it is not there. */
  dir: string;
  /** The same folder as an absolute path. */
  path: string;
  entries: { name: string; path: string; short: string }[];
}

export const canSkip = (p: PresetInfo) => p.agent === 'claude' || p.agent === 'codex';

export type Launch = { ok: true; id: string } | { ok: false; error: string; signedOut?: boolean };

/** Start a session. `preset` is undefined for a plain shell. */
export async function launchSession(req: { preset?: string; yolo?: boolean; dir?: string }): Promise<Launch> {
  const r = await fetch('/api/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ preset: req.preset, yolo: req.yolo ?? false, dir: req.dir }),
  }).catch(() => null);
  if (!r) return { ok: false, error: 'Server unreachable' };
  if (r.status === 401) return { ok: false, error: 'Signed out', signedOut: true };
  if (!r.ok) return { ok: false, error: (await r.text()) || 'That did not work' };
  const { id } = (await r.json()) as { id: string };
  return { ok: true, id };
}

/** Start every session of a preset group. */
export async function launchGroup(name: string): Promise<{ ok: true; ids: string[] } | { ok: false; error: string; signedOut?: boolean }> {
  const r = await fetch(`/api/groups/${encodeURIComponent(name)}/start`, { method: 'POST' }).catch(() => null);
  if (!r) return { ok: false, error: 'Server unreachable' };
  if (r.status === 401) return { ok: false, error: 'Signed out', signedOut: true };
  if (!r.ok) return { ok: false, error: (await r.text()) || 'That did not work' };
  const { ids } = (await r.json()) as { ids: string[] };
  return { ok: true, ids };
}

async function getJson<T>(url: string, fallback: T): Promise<T> {
  const r = await fetch(url, { cache: 'no-store' }).catch(() => null);
  return r?.ok ? ((await r.json()) as T) : fallback;
}

export const fetchPresets = () => getJson<PresetInfo[]>('/api/presets', []);
export const fetchGroups = () => getJson<GroupInfo[]>('/api/groups', []);
export const fetchRecentDirs = () => getJson<RecentDir[]>('/api/dirs', []);
export type NewSessionData = { presets: PresetInfo[]; groups: GroupInfo[]; recents: RecentDir[] };

let newSessionData: NewSessionData | null = null;
let newSessionLoad: Promise<NewSessionData> | null = null;

/** What the new-session sheet needs, if it has been fetched already. */
export const cachedNewSessionData = (): NewSessionData | null => newSessionData;

/** Fetch what the new-session sheet needs and keep it, so the sheet can open on the tap. Calls share one request. */
export function loadNewSessionData(): Promise<NewSessionData> {
  return (newSessionLoad ??= Promise.all([fetchPresets(), fetchGroups(), fetchRecentDirs()])
    .then(([presets, groups, recents]) => (newSessionData = { presets, groups, recents }))
    .finally(() => {
      newSessionLoad = null;
    }));
}

export const browseDirs = (typed: string) =>
  getJson<BrowseResult>(`/api/browse?path=${encodeURIComponent(typed)}`, { dir: '', path: '', entries: [] });

/** The folder `path` with your home folder as `~`, given any other absolute path from the server. */
export function shortPath(path: string, recents: RecentDir[]): string {
  return recents.find((d) => d.path === path)?.short ?? path;
}

/** The two agents the sheet offers up front: the first Claude and the first Codex preset. */
export function mainAgents(presets: PresetInfo[]): { claude?: PresetInfo; codex?: PresetInfo; more: PresetInfo[] } {
  const claude = presets.find((p) => p.agent === 'claude');
  const codex = presets.find((p) => p.agent === 'codex');
  return { claude, codex, more: presets.filter((p) => p !== claude && p !== codex) };
}
