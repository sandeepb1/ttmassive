// Text handling for the composer: spotting a `/command` or `@file` being typed, putting a
// suggestion in its place, and finding uploaded images in a message. Pure, so node can test it.

export interface Trigger {
  kind: 'slash' | 'file';
  /** What was typed after the `/` or `@`. */
  query: string;
  /** The range of the typed word, including its `/` or `@`. */
  start: number;
  end: number;
}

/**
 * Is the cursor at the end of a slash command or an `@file` word?
 * A slash command only counts at the very start of the message; an `@` counts anywhere it
 * begins a word.
 */
export function findTrigger(text: string, cursor: number): Trigger | null {
  const before = text.slice(0, cursor);
  if (/^\/[^\s]*$/.test(before)) {
    return { kind: 'slash', query: before.slice(1), start: 0, end: cursor };
  }
  const m = /(?:^|\s)(@[^\s]*)$/.exec(before);
  if (m) {
    const start = cursor - m[1].length;
    return { kind: 'file', query: m[1].slice(1), start, end: cursor };
  }
  return null;
}

/** `text` with the typed word replaced by `insert`, and where the cursor should go. */
export function applySuggestion(text: string, t: Trigger, insert: string): { text: string; cursor: number } {
  const after = text.slice(t.end);
  // A suggestion ends with a space; don't double it if one already follows.
  const tail = insert.endsWith(' ') && after.startsWith(' ') ? after.slice(1) : after;
  const next = text.slice(0, t.start) + insert + tail;
  return { text: next, cursor: t.start + insert.length };
}

/** The `@path ` token that refers to an attached image. */
export function attachmentToken(path: string): string {
  return `@${path} `;
}

/** Add `token` at `cursor`, with a space before it when it would otherwise touch a word. */
export function insertToken(text: string, cursor: number, token: string): { text: string; cursor: number } {
  const before = text.slice(0, cursor);
  const lead = before === '' || /\s$/.test(before) ? '' : ' ';
  const next = before + lead + token + text.slice(cursor);
  return { text: next, cursor: before.length + lead.length + token.length };
}

/** The text without `token` (and the space that followed it). */
export function removeToken(text: string, token: string): string {
  return text.replace(token, '').replace(token.trimEnd(), '');
}

const UPLOAD = /@(\S*\/uploads\/([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+\.(?:png|jpg|gif|webp)))\s?/g;

/**
 * A message with its attached-image references pulled out: the text that remains, and the URLs
 * the browser can show them from.
 */
export function splitUploads(text: string): { text: string; images: string[] } {
  const images: string[] = [];
  const rest = text.replace(UPLOAD, (_all, _path: string, uid: string, file: string) => {
    images.push(`/api/uploads/${uid}/${file}`);
    return '';
  });
  return { text: rest.trim(), images };
}
