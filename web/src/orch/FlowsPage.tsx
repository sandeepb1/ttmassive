// The page the orchestrator opens on: every flow in a table, with folders as rows. Click a folder to go in, a flow to open
// it in the design view. A search looks in every folder. What a flow is for, how many agents it has and how it last ran are
// columns, so a long list can be told apart without opening anything.

import { useEffect, useMemo, useState } from 'react';
import { browseDirs } from '../dirs';
import { Icon, StatusPill } from './bits';
import { menuOn, useLongPress, type MenuItem, type MenuState } from './ContextMenu';
import { Dialog } from './Dialogs';
import { DEFAULT_SORT, baseOf, childPath, cleanFolderName, listing, moveTargets, parentOf, runsLabel, sortBy, tildePath, whenLabel, type FlowEntry, type FolderRow, type Sort, type SortKey } from './model';
import type { Flow } from './types';

interface Props {
  entries: FlowEntry[];
  folders: string[];
  /** The folders linked to a directory, and the directory each is linked to. */
  folderDirs: Record<string, string>;
  starters: Flow[];
  /** The folder being looked at; empty for the top level. */
  at: string;
  onAt: (path: string) => void;
  onOpen: (e: FlowEntry) => void;
  onNewFlow: () => void;
  onDesign: () => void;
  onUseStarter: (f: Flow) => void;
  /** Make a folder, linked to a directory when one is given. Returns why not, if it could not be. */
  onNewFolder: (path: string, dir?: string) => Promise<string | null>;
  /** Link a folder to a directory, or take the link off with `null`. Returns why not, if it could not be. */
  onLinkFolder: (path: string, dir: string | null) => Promise<string | null>;
  /** A copy of the flow, in the folder, opened to edit. */
  onDuplicateTo: (e: FlowEntry, folder: string) => void;
  onRenameFolder: (from: string, to: string) => void;
  onDeleteFolder: (path: string) => void;
  onMove: (e: FlowEntry, folder: string) => void;
  openMenu: (m: MenuState) => void;
  flowMenu: (e: FlowEntry) => MenuItem[];
}

const COLUMNS: { key: SortKey; label: string; cls: string }[] = [
  { key: 'name', label: 'Name', cls: 'mc-col-name' },
  { key: 'agents', label: 'Agents', cls: 'mc-col-num' },
  { key: 'runs', label: 'Runs', cls: 'mc-col-num' },
  { key: 'last', label: 'Last run', cls: 'mc-col-last' },
];

export function FlowsPage(p: Props) {
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<Sort>(DEFAULT_SORT);
  const [naming, setNaming] = useState<{ rename: string | null } | null>(null);
  const [moving, setMoving] = useState<FlowEntry | null>(null);
  const [duplicating, setDuplicating] = useState<FlowEntry | null>(null);
  // Linking: a new folder made for a directory (`path` null), or an existing folder being linked.
  const [linking, setLinking] = useState<{ path: string | null } | null>(null);
  const [showTemplates, setShowTemplates] = useState(p.entries.length === 0);
  const press = useLongPress(p.openMenu);
  const view = useMemo(() => listing(p.entries, p.folders, p.at, query, sort, p.folderDirs), [p.entries, p.folders, p.at, query, sort, p.folderDirs]);

  const flowItems = (e: FlowEntry): MenuItem[] => {
    const base = p.flowMenu(e);
    return e.saved
      ? [base[0]!, { label: 'Move to a folder…', icon: 'folder', onSelect: () => setMoving(e) }, { label: 'Duplicate to a folder…', icon: 'files', onSelect: () => setDuplicating(e) }, ...base.slice(1)]
      : base;
  };
  const folderItems = (f: FolderRow): MenuItem[] => [
    { label: 'Open', icon: 'folder', onSelect: () => p.onAt(f.path) },
    { label: 'Rename', icon: 'edit', onSelect: () => setNaming({ rename: f.path }) },
    { label: f.dir ? 'Change the directory…' : 'Link to a directory…', icon: 'branch', onSelect: () => setLinking({ path: f.path }) },
    ...(f.dir ? [{ label: 'Remove the link', icon: 'minus' as const, onSelect: () => void p.onLinkFolder(f.path, null) }] : []),
    { kind: 'separator' },
    { label: f.flows ? 'Delete folder (move its flows out first)' : 'Delete folder', icon: 'trash', danger: true, disabled: f.flows > 0, onSelect: () => p.onDeleteFolder(f.path) },
  ];
  const more = (items: () => MenuItem[]) => (e: React.MouseEvent<HTMLButtonElement>) => {
    e.stopPropagation();
    const r = e.currentTarget.getBoundingClientRect();
    p.openMenu({ x: r.right, y: r.bottom, items: items() });
  };

  const nothing = view.folders.length === 0 && view.flows.length === 0 && !naming;

  return (
    <div className="mc-page">
      <div className="mc-page-head">
        <nav className="mc-crumbs" aria-label="Folder">
          {view.crumbs.map((c, i) => (
            <span key={c.path} className="mc-crumb">
              {i > 0 && <span aria-hidden="true">›</span>}
              {i === view.crumbs.length - 1 ? (
                <h1 aria-current="page">{c.name}</h1>
              ) : (
                <button type="button" className="mc-anchor" onClick={() => p.onAt(c.path)}>
                  {c.name}
                </button>
              )}
            </span>
          ))}
        </nav>
        <span className="mc-spacer" />
        <button type="button" className="mc-btn" onClick={() => setNaming({ rename: null })}>
          <Icon name="folder" /> <span className="mc-hide-narrow">New folder</span>
        </button>
        <button type="button" className="mc-btn" onClick={() => setLinking({ path: null })} title={view.linked ? 'A project is a directory inside the linked one; its flows run there' : 'Make a folder for a directory on this machine; the flows kept in it run there'}>
          <Icon name="branch" /> <span className="mc-hide-narrow">{view.linked ? 'Add a project' : 'Link a directory'}</span>
        </button>
        <button type="button" className="mc-btn" onClick={p.onDesign} title="Describe the kind of work and let Sonnet design a reusable team">
          <Icon name="wand" /> Design a flow
        </button>
        <button type="button" className="mc-btn mc-btn-primary" onClick={p.onNewFlow}>
          <Icon name="plus" /> New flow
        </button>
      </div>

      {view.linked && !view.searching && (
        <p className="mc-linkbar" role="note">
          <Icon name="branch" />
          <span>
            Runs of the flows kept here use <b className="mc-mono">{tildePath(view.linked.dir)}</b>
            {view.linked.folder !== p.at ? ` (linked from “${view.linked.folder}”)` : ''}. To run one somewhere else, duplicate it into another folder.
          </span>
        </p>
      )}

      <div className="mc-page-tools">
        <input className="mc-field" type="search" value={query} placeholder="Search flows by name, purpose or folder" aria-label="Search flows" onChange={(e) => setQuery(e.target.value)} />
      </div>

      <table className="mc-table" aria-label="Flows">
        <thead>
          <tr>
            {COLUMNS.slice(0, 1).map((c) => (
              <th key={c.key} className={c.cls} aria-sort={sort.key === c.key ? (sort.dir === 1 ? 'ascending' : 'descending') : 'none'}>
                <button type="button" className="mc-th" onClick={() => setSort(sortBy(sort, c.key))}>
                  {c.label} {sort.key === c.key ? (sort.dir === 1 ? '↑' : '↓') : ''}
                </button>
              </th>
            ))}
            <th className="mc-col-purpose">What it is for</th>
            {COLUMNS.slice(1).map((c) => (
              <th key={c.key} className={c.cls} aria-sort={sort.key === c.key ? (sort.dir === 1 ? 'ascending' : 'descending') : 'none'}>
                <button type="button" className="mc-th" onClick={() => setSort(sortBy(sort, c.key))}>
                  {c.label} {sort.key === c.key ? (sort.dir === 1 ? '↑' : '↓') : ''}
                </button>
              </th>
            ))}
            <th className="mc-col-act">
              <span className="mc-sronly">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {naming && naming.rename === null && (
            <tr className="mc-tr">
              <td colSpan={6}>
                <NameField initial="" label="New folder name" onDone={(name) => (setNaming(null), name && void p.onNewFolder(childPath(p.at, name)))} />
              </td>
            </tr>
          )}
          {view.folders.map((f) => (
            <tr key={f.path} className="mc-tr mc-tr-folder" {...menuOn(p.openMenu, () => folderItems(f))} {...press(() => folderItems(f))}>
              <td className="mc-col-name">
                {naming?.rename === f.path ? (
                  <NameField initial={f.path.split('/').pop() ?? ''} label={`Rename ${f.path}`} onDone={(name) => (setNaming(null), name && name !== f.path.split('/').pop() && p.onRenameFolder(f.path, childPath(parentOf(f.path), name)))} />
                ) : (
                  <button type="button" className="mc-anchor mc-cellname" onClick={() => p.onAt(f.path)}>
                    <Icon name="folder" /> {f.path.split('/').pop()}
                  </button>
                )}
              </td>
              <td className="mc-col-purpose mc-muted">
                {f.flows} {f.flows === 1 ? 'flow' : 'flows'}
                {f.folders ? ` · ${f.folders} ${f.folders === 1 ? 'folder' : 'folders'}` : ''}
                {f.dir && <span className="mc-dirtag mc-mono" title={f.dir}>→ {tildePath(f.dir)}</span>}
              </td>
              <td className="mc-col-num" />
              <td className="mc-col-num" />
              <td className="mc-col-last" />
              <td className="mc-col-act">
                <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Actions for the folder ${f.path}`} onClick={more(() => folderItems(f))}>
                  <Icon name="more" />
                </button>
              </td>
            </tr>
          ))}
          {view.flows.map((e) => (
            <tr key={e.id} className="mc-tr" {...menuOn(p.openMenu, () => flowItems(e))} {...press(() => flowItems(e))}>
              <td className="mc-col-name">
                <button type="button" className="mc-anchor mc-cellname" onClick={() => p.onOpen(e)}>
                  <Icon name="workflow" /> {e.name || 'Untitled flow'}
                </button>
                {!e.saved && (
                  <span className="mc-pill" data-tone="idle" title="Only its executions are left">
                    runs only
                  </span>
                )}
                {view.searching && e.folder && <span className="mc-where">{e.folder.replace(/\//g, ' › ')}</span>}
              </td>
              <td className="mc-col-purpose">
                <div className="mc-clamp">{e.purpose}</div>
              </td>
              <td className="mc-col-num">{e.agents ?? '—'}</td>
              <td className="mc-col-num" title={runsLabel(e.runs.length)}>
                {e.runs.length}
              </td>
              <td className="mc-col-last">
                {e.last ? (
                  <>
                    <StatusPill status={e.last.status} /> <span className="mc-muted">{whenLabel(e.last.created)}</span>
                  </>
                ) : (
                  <span className="mc-muted">never run</span>
                )}
              </td>
              <td className="mc-col-act">
                <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label={`Actions for ${e.name}`} onClick={more(() => flowItems(e))}>
                  <Icon name="more" />
                </button>
              </td>
            </tr>
          ))}
          {nothing && (
            <tr>
              <td colSpan={6} className="mc-empty">
                {query.trim() ? 'No flow matches that.' : p.at ? 'This folder is empty. Open a flow and move it here, or make a new one.' : 'No flows yet. Design one from a description, start from a template below, or build one.'}
              </td>
            </tr>
          )}
        </tbody>
      </table>

      <section className="mc-templates" aria-label="Templates">
        <button type="button" className="mc-anchor mc-templates-head" aria-expanded={showTemplates} onClick={() => setShowTemplates((s) => !s)}>
          <Icon name={showTemplates ? 'chevron-down' : 'chevron'} /> Templates
        </button>
        {showTemplates && (
          <ul className="mc-template-list">
            {p.starters.map((f) => (
              <li key={f.id}>
                <button type="button" className="mc-template" onClick={() => p.onUseStarter(f)}>
                  <span className="mc-cellname">{f.name}</span>
                  <span className="mc-muted">{f.purpose}</span>
                  <span className="mc-muted">{f.nodes.length} agents · opens as a copy to edit</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {moving && (
        <MoveDialog
          entry={moving}
          folders={p.folders}
          verb="Move"
          onMove={(folder) => (setMoving(null), folder !== moving.folder && p.onMove(moving, folder))}
          onClose={() => setMoving(null)}
        />
      )}
      {duplicating && (
        <MoveDialog entry={duplicating} folders={p.folders} verb="Duplicate" onMove={(folder) => (setDuplicating(null), p.onDuplicateTo(duplicating, folder))} onClose={() => setDuplicating(null)} />
      )}
      {linking && (
        <LinkDialog
          parent={view.linked}
          at={p.at}
          path={linking.path}
          current={linking.path ? (p.folderDirs[linking.path] ?? '') : ''}
          onSubmit={async (name, dir) => {
            const why = linking.path ? await p.onLinkFolder(linking.path, dir) : await p.onNewFolder(childPath(p.at, name), dir);
            if (!why) setLinking(null);
            return why;
          }}
          onClose={() => setLinking(null)}
        />
      )}
    </div>
  );
}

/** A name typed in place: Enter keeps it, Escape or leaving the field drops it. */
function NameField({ initial, label, onDone }: { initial: string; label: string; onDone: (name: string) => void }) {
  const [text, setText] = useState(initial);
  return (
    <input
      className="mc-field mc-name-field"
      autoFocus
      value={text}
      maxLength={40}
      aria-label={label}
      placeholder="Folder name"
      onChange={(e) => setText(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') onDone(cleanFolderName(text));
        if (e.key === 'Escape') onDone('');
      }}
      onBlur={() => onDone('')}
    />
  );
}

function MoveDialog({ entry, folders, verb, onMove, onClose }: { entry: FlowEntry; folders: string[]; verb: 'Move' | 'Duplicate'; onMove: (folder: string) => void; onClose: () => void }) {
  const targets = moveTargets(folders);
  const [target, setTarget] = useState(entry.folder);
  const [fresh, setFresh] = useState('');
  const name = cleanFolderName(fresh);
  const dest = name ? childPath(target, name) : target;
  return (
    <Dialog
      title={`${verb} “${entry.name}”`}
      onClose={onClose}
      foot={
        <>
          <button type="button" className="mc-btn" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="mc-btn mc-btn-primary" disabled={verb === 'Move' && dest === entry.folder} onClick={() => onMove(dest)}>
            {verb}
          </button>
        </>
      }
    >
      <div className="mc-section">
        <label className="mc-label" htmlFor="mc-move-to">
          {verb === 'Move' ? 'Move to' : 'Put the copy in'}
        </label>
        <select id="mc-move-to" className="mc-field" value={target} onChange={(e) => setTarget(e.target.value)}>
          {targets.map((t) => (
            <option key={t.path} value={t.path}>
              {t.label}
            </option>
          ))}
        </select>
      </div>
      <div className="mc-section">
        <label className="mc-label" htmlFor="mc-move-new">
          Or in a new folder inside it
        </label>
        <input id="mc-move-new" className="mc-field" value={fresh} maxLength={40} placeholder="Folder name (optional)" onChange={(e) => setFresh(e.target.value)} />
        <p className="mc-hint">{dest ? `It will be in ${dest.replace(/\//g, ' › ')}.` : 'It will be at the top level.'}</p>
      </div>
    </Dialog>
  );
}

/** A directory typed in, with the folders inside what has been typed offered to click, so a long path is not typed in full. */
function DirectoryField({ value, onChange, label }: { value: string; onChange: (v: string) => void; label: string }) {
  const [options, setOptions] = useState<{ name: string; short: string }[]>([]);
  useEffect(() => {
    let stale = false;
    const id = window.setTimeout(async () => {
      const r = await browseDirs(value);
      if (!stale) setOptions(r.entries.slice(0, 10));
    }, 200);
    return () => {
      stale = true;
      window.clearTimeout(id);
    };
  }, [value]);
  return (
    <>
      <input className="mc-field mc-mono" value={value} aria-label={label} placeholder="~/code/project" autoFocus onChange={(e) => onChange(e.target.value)} />
      {options.length > 0 && (
        <div className="mc-chips" style={{ marginTop: '0.5rem' }}>
          {options.map((o) => (
            <button key={o.short} type="button" className="mc-chip" onClick={() => onChange(`${o.short}/`)}>
              {o.name}
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** Make a folder for a directory (a project, when it is made inside a linked folder), or link a folder that is already there. */
function LinkDialog({ parent, at, path, current, onSubmit, onClose }: {
  parent: { folder: string; dir: string } | null;
  at: string;
  path: string | null;
  current: string;
  onSubmit: (name: string, dir: string) => Promise<string | null>;
  onClose: () => void;
}) {
  const [dir, setDir] = useState(current ? tildePath(current) : parent ? `${tildePath(parent.dir)}/` : '~/');
  const [name, setName] = useState('');
  const [named, setNamed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [why, setWhy] = useState<string | null>(null);
  const fromDir = cleanFolderName(baseOf(dir.replace(/\/+$/, '')).replace(/^~$/, ''));
  const shown = named ? name : fromDir;
  const chosen = cleanFolderName(shown);
  const ready = dir.trim().length > 1 && (path !== null || chosen.length > 0) && !busy;
  const go = async () => {
    setBusy(true);
    setWhy(null);
    const err = await onSubmit(chosen, dir.trim());
    setBusy(false);
    if (err) setWhy(err);
  };
  const title = path ? `Link “${baseOf(path)}” to a directory` : parent ? 'Add a project' : 'Link a directory';
  return (
    <Dialog
      title={title}
      onClose={busy ? undefined : onClose}
      foot={
        <>
          <button type="button" className="mc-btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="mc-btn mc-btn-primary" disabled={!ready} onClick={() => void go()}>
            {path ? 'Link' : parent ? 'Add project' : 'Link'}
          </button>
        </>
      }
    >
      <div className="mc-section">
        <label className="mc-label">Directory</label>
        <DirectoryField value={dir} onChange={setDir} label="Directory" />
        <p className="mc-hint">
          {parent ? `A project is a directory inside ${tildePath(parent.dir)}.` : 'A directory on this machine.'} Flows kept in the folder run there, and only there; to run one somewhere else, duplicate it into another folder.
        </p>
      </div>
      {path === null && (
        <div className="mc-section">
          <label className="mc-label" htmlFor="mc-link-name">
            Folder name
          </label>
          <input
            id="mc-link-name"
            className="mc-field"
            value={shown}
            maxLength={40}
            placeholder="Name of the folder"
            onChange={(e) => (setNamed(true), setName(e.target.value))}
          />
          <p className="mc-hint">{chosen ? `It will be ${childPath(at, chosen).replace(/\//g, ' › ')}.` : 'It takes the directory’s name unless you give it another.'}</p>
        </div>
      )}
      {why && <p className="mc-error" role="alert">{why}</p>}
    </Dialog>
  );
}
