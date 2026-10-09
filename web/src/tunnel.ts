// The public address ttm was given when it started with `--tunnel`, shown in Settings.
// Nothing here when the server has no tunnel.

export interface TunnelInfo {
  provider: string | null;
  url: string | null;
  up_secs: number | null;
  reconnects: number;
  last_error: string | null;
  /** The free Pinggy tunnel closes after an hour, so the address will change. */
  expires: boolean;
}

/** The tunnel as the server reports it, or null when there is none (or we cannot tell). */
export async function fetchTunnel(): Promise<TunnelInfo | null> {
  try {
    const r = await fetch('/api/tunnel');
    if (!r.ok) return null;
    const t = (await r.json()) as TunnelInfo | null;
    return t && t.provider ? t : null;
  } catch {
    return null;
  }
}

/** One line for the settings sheet: the state of the tunnel, without the address. */
export function tunnelStatus(t: TunnelInfo): string {
  if (!t.url) return t.last_error ? `Connecting… (${t.last_error})` : 'Connecting…';
  const via = t.provider ?? 'tunnel';
  const note = t.expires ? '. The free tier closes after an hour and the address changes' : '';
  return `Via ${via}${note}`;
}
