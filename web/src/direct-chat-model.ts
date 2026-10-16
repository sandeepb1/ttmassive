// A conversation with a model on an account, with no agent CLI. The page keeps it and sends it whole each time; these are
// the parts that can be tested without a screen: what is kept, what is sent, and what it has cost.

export interface ChatMsg {
  role: 'user' | 'assistant';
  content: string;
  /** What a reply cost in dollars, when the account knew. */
  costUsd?: number | null;
  tokens?: { in: number; out: number };
  /** Web tools the model used for this reply. */
  tools?: string[];
}

export interface DirectChat {
  account: string;
  model: string;
  web: boolean;
  messages: ChatMsg[];
}

/** Most messages kept for a conversation. */
export const KEEP = 60;

export const chatKey = (account: string, model: string): string => `ttm.directchat.${account}.${model}`;

/** The messages as the server wants them: roles and text only. */
export const toWire = (messages: ChatMsg[]): { role: string; content: string }[] => messages.map(({ role, content }) => ({ role, content }));

/** The conversation with a message added, the oldest dropped when it gets long (in pairs, so it still starts with the person). */
export function push(messages: ChatMsg[], m: ChatMsg): ChatMsg[] {
  const next = [...messages, m];
  let cut = Math.max(0, next.length - KEEP);
  if (cut % 2 === 1) cut += 1;
  const kept = next.slice(cut);
  return kept[0]?.role === 'assistant' ? kept.slice(1) : kept;
}

/** What the conversation has cost so far, and whether some replies had no price to work it out from. */
export function spent(messages: ChatMsg[]): { usd: number; unknown: number } {
  let usd = 0;
  let unknown = 0;
  for (const m of messages) {
    if (m.role !== 'assistant') continue;
    if (typeof m.costUsd === 'number') usd += m.costUsd;
    else unknown += 1;
  }
  return { usd, unknown };
}

/** "$0.0021", with a note when some replies could not be priced. */
export function spentLine(messages: ChatMsg[]): string {
  const { usd, unknown } = spent(messages);
  const money = usd > 0 ? (usd < 0.01 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(2)}`) : '$0.00';
  return unknown > 0 ? `${money} so far (${unknown} ${unknown === 1 ? 'reply' : 'replies'} not priced)` : `${money} so far`;
}

/** Read a saved conversation, or an empty one if it is missing or not what was saved. */
export function parseSaved(raw: string | null): ChatMsg[] {
  try {
    const v: unknown = JSON.parse(raw ?? '[]');
    if (!Array.isArray(v)) return [];
    return v.filter((m): m is ChatMsg => !!m && typeof m === 'object' && ((m as ChatMsg).role === 'user' || (m as ChatMsg).role === 'assistant') && typeof (m as ChatMsg).content === 'string').slice(-KEEP);
  } catch {
    return [];
  }
}
