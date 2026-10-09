// Web Push subscription management (page side).

export type PushState = 'unsupported' | 'denied' | 'off' | 'on';

function b64urlToBytes(s: string): Uint8Array<ArrayBuffer> {
  const pad = '='.repeat((4 - (s.length % 4)) % 4);
  const raw = atob((s + pad).replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

function supported(): boolean {
  return 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;
}

async function registration(): Promise<ServiceWorkerRegistration> {
  return navigator.serviceWorker.ready;
}

export async function pushState(): Promise<PushState> {
  if (!supported()) return 'unsupported';
  if (Notification.permission === 'denied') return 'denied';
  const sub = await (await registration()).pushManager.getSubscription();
  return sub && Notification.permission === 'granted' ? 'on' : 'off';
}

/** Must be called from a user gesture (permission prompt). Throws a readable Error. */
export async function enablePush(): Promise<void> {
  if (!supported()) throw new Error('This browser does not support push notifications');
  if ((await Notification.requestPermission()) !== 'granted') throw new Error('Notification permission was not granted');

  const keyRes = await fetch('/api/push/key');
  if (!keyRes.ok) throw new Error('Could not fetch the server push key');
  const { key } = (await keyRes.json()) as { key: string };

  const reg = await registration();
  const wanted = b64urlToBytes(key);
  let sub = await reg.pushManager.getSubscription();
  // A subscription is bound to the server key it was made with. If the server's key changed
  // (e.g. its data directory was recreated), the old one can never deliver: replace it.
  const have = sub?.options.applicationServerKey;
  if (sub && have && !sameBytes(new Uint8Array(have), wanted)) {
    await sub.unsubscribe();
    sub = null;
  }
  sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: wanted });
  const r = await fetch('/api/push/subscribe', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(sub.toJSON()),
  });
  if (!r.ok) throw new Error(`Server rejected the subscription: ${await r.text()}`);
}

export async function disablePush(): Promise<void> {
  const sub = await (await registration()).pushManager.getSubscription();
  if (!sub) return;
  await fetch('/api/push/unsubscribe', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ endpoint: sub.endpoint }),
  }).catch(() => null);
  await sub.unsubscribe();
}

export async function sendTest(session: string): Promise<number> {
  const r = await fetch('/api/push/test', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ session }),
  });
  if (!r.ok) throw new Error('Test failed');
  return ((await r.json()) as { devices: number }).devices;
}
