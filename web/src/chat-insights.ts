// Summaries computed from the conversation: which files changed, and a search through it.
// Pure, so node can test them.

import type { ChatItem, EditPair } from './chat-model';
import { lineDiff } from './diff.ts';

export interface ChangedFile {
  path: string;
  /** The file's own name. */
  name: string;
  /** The folder it is in, as the last two parts, or ''. */
  dir: string;
  edits: EditPair[];
  added: number;
  removed: number;
  /** The first thing done to it was writing it fresh. */
  created: boolean;
  deleted: boolean;
  /** An edit is still running. */
  pending: boolean;
  /** Edits the agent tried that failed or were refused. */
  failed: number;
  /** The sequence number of the latest change, for ordering. */
  seq: number;
}

const counts = new WeakMap<EditPair, { added: number; removed: number }>();

function count(e: EditPair): { added: number; removed: number } {
  let c = counts.get(e);
  if (!c) {
    c = { added: 0, removed: 0 };
    // A file's closing newline ends its last line; it is not a line of its own.
    const trim = (t: string) => (t.endsWith('\n') ? t.slice(0, -1) : t);
    for (const l of lineDiff(trim(e.old), trim(e.new))) {
      if (l.t === 'add') c.added++;
      else if (l.t === 'del') c.removed++;
    }
    counts.set(e, c);
  }
  return c;
}

export function splitPath(path: string): { name: string; dir: string } {
  const parts = path.split('/').filter(Boolean);
  const name = parts.pop() ?? path;
  return { name, dir: parts.slice(-2).join('/') };
}

/** Every file the agent edited, wrote or deleted, most recently changed first, edits merged. */
export function filesChanged(items: ChatItem[]): ChangedFile[] {
  const byPath = new Map<string, ChangedFile>();
  for (const it of items) {
    if (it.kind !== 'tool' || !it.diff) continue;
    const { path } = it.diff;
    let f = byPath.get(path);
    if (!f) {
      f = { path, ...splitPath(path), edits: [], added: 0, removed: 0, created: false, deleted: false, pending: false, failed: 0, seq: 0 };
      byPath.set(path, f);
    }
    if (it.status === 'error') {
      f.failed++;
      continue;
    }
    if (f.edits.length === 0 && it.tool === 'Write' && it.diff.edits[0]?.old === '') f.created = true;
    f.deleted = it.tool === 'Delete';
    f.pending = it.status === 'running';
    f.seq = Math.max(f.seq, it.seq);
    for (const e of it.diff.edits) {
      f.edits.push(e);
      const c = count(e);
      f.added += c.added;
      f.removed += c.removed;
    }
  }
  // A file the agent only failed to touch is not a change.
  return [...byPath.values()].filter((f) => f.edits.length > 0).sort((a, b) => b.seq - a.seq);
}

export interface Hit {
  id: string;
  kind: ChatItem['kind'];
  before: string;
  match: string;
  after: string;
}

const BEFORE = 36;
const AFTER = 64;

function textOf(it: ChatItem): string {
  switch (it.kind) {
    case 'user':
    case 'assistant':
    case 'thinking':
    case 'command':
    case 'notice':
      return it.text;
    case 'tool':
      return [it.summary, it.input ?? '', it.output ?? ''].join('\n');
    case 'question':
      return it.questions.map((q) => `${q.question} ${q.options.map((o) => o.label).join(' ')}`).join('\n');
  }
}

/** Items containing `query` (case-insensitive), newest first, each with the words around the first match. */
export function searchItems(items: ChatItem[], query: string, limit = 60): Hit[] {
  const q = query.trim().toLowerCase();
  if (q.length < 2) return [];
  const hits: Hit[] = [];
  for (let i = items.length - 1; i >= 0 && hits.length < limit; i--) {
    const it = items[i]!;
    const flat = textOf(it).replace(/\s+/g, ' ').trim();
    const at = flat.toLowerCase().indexOf(q);
    if (at < 0) continue;
    const start = Math.max(0, at - BEFORE);
    const end = Math.min(flat.length, at + q.length + AFTER);
    hits.push({
      id: it.id,
      kind: it.kind,
      before: (start > 0 ? '…' : '') + flat.slice(start, at),
      match: flat.slice(at, at + q.length),
      after: flat.slice(at + q.length, end) + (end < flat.length ? '…' : ''),
    });
  }
  return hits;
}
