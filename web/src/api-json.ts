// JSON calls for the sidebar tabs, which fetch their own data. A failure comes back as a message
// to show rather than an exception, and a 401 is passed on so the page can show the login.

export type Fetched<T> = { ok: true; data: T } | { ok: false; status: number; error: string };

export async function callJson<T>(url: string, init: RequestInit | undefined, signedOut: () => void): Promise<Fetched<T>> {
  let r: Response;
  try {
    r = await fetch(url, { cache: 'no-store', ...init });
  } catch {
    return { ok: false, status: 0, error: 'Server unreachable' };
  }
  if (r.status === 401) {
    signedOut();
    return { ok: false, status: 401, error: 'Signed out' };
  }
  if (!r.ok) {
    const text = await r.text().catch(() => '');
    return { ok: false, status: r.status, error: text.trim() || `The server answered ${r.status}` };
  }
  try {
    return { ok: true, data: (await r.json()) as T };
  } catch {
    return { ok: false, status: r.status, error: 'The server’s reply could not be read' };
  }
}

export const getJson = <T>(url: string, signedOut: () => void) => callJson<T>(url, undefined, signedOut);

export const postJson = <T>(url: string, body: unknown, signedOut: () => void) =>
  callJson<T>(
    url,
    { method: 'POST', headers: { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) },
    signedOut,
  );

/** `/api/sessions/{id}/{path}` with the id escaped. */
export const sessionUrl = (session: string, path: string) => `/api/sessions/${encodeURIComponent(session)}/${path}`;
