// Updating the server from its GitHub release: the toast that offers it, the sheet that shows what changed,
// the install and the wait for the restarted server, and the block in Settings. The server does the work
// (see src/selfupdate.rs); a client that is behind the new server then reloads itself (update.ts).

import { el } from './dom';
import { renderMarkdown } from './chat-render';
import { arrived, checkedAgo, headline, offerText, type ServerUpdate } from './server-update-model';
import { button } from './ui/button';
import { sheetShell } from './ui/sheet';
import { sheetActions, sheetBlock, sheetHint, sheetLabel } from './ui/sheet-parts';
import { toast } from './ui/toast';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** The server's view of releases, or null when it cannot be reached. `refresh` makes it ask GitHub again. */
export async function fetchUpdate(refresh = false): Promise<ServerUpdate | null> {
  try {
    const r = await fetch(`/api/update${refresh ? '?refresh=1' : ''}`);
    return r.ok ? ((await r.json()) as ServerUpdate) : null;
  } catch {
    return null;
  }
}

/** Ask the server to install `version`, then wait for it to come back as that version and reload. */
export async function installUpdate(host: HTMLElement, version: string): Promise<void> {
  const working = toast(host, `Updating to ttm v${version}…`, { ms: 180_000 });
  const r = await fetch('/api/update', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ version }),
  }).catch(() => null);
  if (!r?.ok) {
    working();
    toast(host, r ? (await r.text()) || 'The update did not start' : 'The server could not be reached', { ms: 10_000 });
    return;
  }
  const until = Date.now() + 120_000;
  while (Date.now() < until) {
    await sleep(1500);
    const u = await fetchUpdate();
    if (!u) continue; // restarting
    if (arrived(u, version)) {
      location.reload();
      return;
    }
    if (u.lastResult && !u.lastResult.ok && u.lastResult.to === version) {
      working();
      toast(host, `ttm v${version} did not start, so v${u.current} was restored.`, { ms: 15_000 });
      return;
    }
  }
  working();
  toast(host, 'The server has not come back yet. Reload when it has.', { ms: 600_000, action: { label: 'Reload', run: () => location.reload() } });
}

/** A sheet with the notes of every release you have not installed, newest first. */
export function openChangelog(host: HTMLElement, u: ServerUpdate): void {
  const shell = sheetShell('What’s new', () => shell.remove());
  const latest = u.releases[0];
  for (const r of u.releases) {
    const head = el('div', 'flex flex-wrap items-baseline justify-between gap-2');
    head.append(el('strong', 'text-md', `v${r.version}${headline(r) ? ` — ${headline(r)}` : ''}`));
    if (r.url) {
      const a = el('a', 'text-sm text-accent-text', 'GitHub');
      a.href = r.url;
      a.target = '_blank';
      a.rel = 'noopener';
      head.append(a);
    }
    shell.body.append(sheetBlock(head, r.notes.trim() ? renderMarkdown(r.notes) : sheetHint('No notes.')));
  }
  if (latest) {
    const actions = sheetActions();
    if (u.canInstall) {
      actions.append(
        button({ onClick: () => { shell.remove(); void installUpdate(host, latest.version); } }, `Update to v${latest.version}`),
      );
    } else if (u.cannotInstallReason) {
      shell.body.append(sheetHint(u.cannotInstallReason));
    }
    shell.body.append(actions);
  }
  shell.open(host);
}

/** The toast: what is available, what it is about, and the two things you can do about it. */
export function offerServerUpdate(host: HTMLElement, u: ServerUpdate): void {
  const text = offerText(u);
  const latest = u.releases[0];
  if (!text || !latest) return;
  toast(host, text, {
    ms: 600_000,
    actions: [
      { label: 'What’s new', run: (_b, dismiss) => { dismiss(); openChangelog(host, u); } },
      ...(u.canInstall ? [{ label: 'Update', run: (_b: HTMLButtonElement, dismiss: () => void) => { dismiss(); void installUpdate(host, latest.version); } }] : []),
    ],
  });
}

/** The Updates section of Settings: the running version, a way to look now, and the update when there is one. */
export function updatesBlock(host: HTMLElement, close: () => void): HTMLElement {
  const block = sheetBlock(sheetLabel('Updates'));
  const draw = (u: ServerUpdate | null, checking = false) => {
    block.replaceChildren(sheetLabel('Updates'));
    if (!u) {
      block.append(sheetHint('The server could not be reached.'));
      return;
    }
    const latest = u.releases[0];
    const line = el('div', 'flex items-center justify-between gap-3');
    line.append(el('span', 'text-md', `ttm v${u.current}`), el('span', 'text-sm text-muted-foreground', latest ? `v${latest.version} available` : 'Up to date'));
    block.append(line, sheetHint(u.error ? `Could not reach GitHub: ${u.error}` : checkedAgo(u.checkedAt, Date.now() / 1000)));
    const actions = sheetActions();
    actions.append(
      button({ variant: 'secondary', size: 'sm', disabled: checking, onClick: async () => { draw(u, true); draw(await fetchUpdate(true)); } }, checking ? 'Checking…' : 'Check now'),
    );
    if (latest) {
      actions.append(button({ variant: 'secondary', size: 'sm', onClick: () => openChangelog(host, u) }, 'What’s new'));
      if (u.canInstall) actions.append(button({ size: 'sm', onClick: () => { close(); void installUpdate(host, latest.version); } }, `Update to v${latest.version}`));
    }
    block.append(actions);
    if (latest && !u.canInstall && u.cannotInstallReason) block.append(sheetHint(u.cannotInstallReason));
  };
  void fetchUpdate().then((u) => draw(u));
  return block;
}
