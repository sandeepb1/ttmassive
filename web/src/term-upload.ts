// Uploading a file pasted or dropped into a terminal pane.

import { prepareImage } from './image';

/** Upload one file for `session` and return the absolute path it was saved at on the host. */
export async function uploadFile(session: string, file: File): Promise<string> {
  const body = file.type.startsWith('image/') ? await prepareImage(file).catch(() => file) : file;
  const r = await fetch(`/api/sessions/${encodeURIComponent(session)}/upload?name=${encodeURIComponent(file.name)}`, {
    method: 'POST',
    headers: { 'content-type': body.type || 'application/octet-stream' },
    body,
  });
  if (r.status === 401) throw new UnauthorizedError();
  if (!r.ok) throw new Error((await r.text()) || `Could not upload ${file.name || 'the file'}`);
  return ((await r.json()) as { path: string }).path;
}

export class UnauthorizedError extends Error {}
