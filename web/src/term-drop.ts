// Files pasted or dropped into a terminal pane. The CLIs read an image from the *server's*
// clipboard (Claude's Ctrl+V), which a browser cannot reach, so the file is uploaded and its path is
// pasted into the prompt instead. Both Claude Code and Codex attach an image given its path.

/** Quote `path` for a shell prompt only when it needs it. */
export function shellQuote(path: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(path)) return path;
  return `'${path.replaceAll("'", "'\\''")}'`;
}

/** The text to paste for these uploaded paths: quoted, one space apart, and a trailing space. */
export function pathsToPaste(paths: string[]): string {
  return paths.map(shellQuote).join(' ') + ' ';
}

/** The files carried by a paste or drop. Text-only transfers return none, so xterm handles them. */
export function filesOf(data: DataTransfer | null): File[] {
  return data ? Array.from(data.files) : [];
}
