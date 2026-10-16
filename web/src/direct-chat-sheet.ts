// Talk to a model on one of your accounts with no agent CLI: for an OpenAI-compatible endpoint, which no CLI can run.
// The conversation lives in this browser (it is the page that keeps it); each message sends it whole, and each reply says what
// it cost. A model can be given web search through your web accounts.

import { callJson, getJson } from './api-json';
import { isWeb, type Account } from './accounts-model';
import { KEEP, chatKey, parseSaved, push, spentLine, toWire, type ChatMsg } from './direct-chat-model';
import { el } from './dom';
import { Sheet } from './sheet';
import { button } from './ui/button';
import { chip, setChipOn } from './ui/chip';
import { select, textarea } from './ui/input';
import { sheetHint, sheetMsg } from './ui/sheet-parts';

interface Reply {
  text: string;
  tokensIn: number;
  tokensOut: number;
  costUsd: number | null;
  tools: { name: string }[];
  credits: number;
  /** What those credits cost, in dollars. */
  webUsd?: number;
}

function load(key: string): ChatMsg[] {
  try {
    return parseSaved(localStorage.getItem(key));
  } catch {
    return [];
  }
}

function save(key: string, m: ChatMsg[]) {
  try {
    localStorage.setItem(key, JSON.stringify(m.slice(-KEEP)));
  } catch {
    /* the conversation just is not kept */
  }
}

export class DirectChatSheet extends Sheet {
  private accounts: Account[] = [];
  private account = '';
  private model = '';
  private web = false;
  private messages: ChatMsg[] = [];
  private log = el('div', 'flex flex-col gap-2');
  private cost = sheetHint('');
  private input = textarea({ rows: 2, placeholder: 'Message', label: 'Message' });
  private send = button({}, 'Send');
  private pickers = el('div', 'flex flex-wrap items-center gap-2');
  private hasWeb = false;

  constructor(host: HTMLElement, private unauthorized: () => void) {
    super(host, '', 'Chat with a model', unauthorized);
  }

  async open() {
    this.tall();
    this.message('Loading…');
    this.show();
    const [accts, models] = await Promise.all([getJson<Account[]>('/api/accounts', this.unauthorized), getJson<{ web?: boolean }>('/api/orch/models', this.unauthorized)]);
    if (this.gone) return;
    this.accounts = accts.ok ? accts.data.filter((a) => !isWeb(a.kind) && a.hasKey && a.models.length > 0) : [];
    this.hasWeb = !!(models.ok && models.data.web);
    if (this.accounts.length === 0) return this.message('No account with pinned models and a key. Add one in Settings, under Account, and pin a model.');
    this.account = this.accounts[0]!.id;
    this.model = this.accounts[0]!.models[0]!.id;
    this.build();
    this.restore();
  }

  private build() {
    this.input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        void this.ask();
      }
    });
    this.send.addEventListener('click', () => void this.ask());
    const clear = button({ variant: 'ghost', size: 'sm', onClick: () => this.reset() }, 'New conversation');
    const scroller = el('div', 'min-h-0 flex-1 overflow-y-auto');
    scroller.append(this.log);
    const bar = el('div', 'flex items-end gap-2');
    bar.append(this.input, this.send);
    const wrap = el('div', 'flex min-h-0 flex-1 flex-col gap-3');
    wrap.append(this.pickers, scroller, this.cost, bar, clear);
    this.body.replaceChildren(wrap);
    this.drawPickers();
  }

  private drawPickers() {
    const acct = this.accounts.find((a) => a.id === this.account)!;
    const accountSel = select(this.accounts.map((a) => ({ value: a.id, label: a.name })), { value: this.account, label: 'Account', cls: 'w-auto' });
    accountSel.addEventListener('change', () => {
      this.account = accountSel.value;
      this.model = this.accounts.find((a) => a.id === this.account)!.models[0]!.id;
      this.drawPickers();
      this.restore();
    });
    const modelSel = select(acct.models.map((m) => ({ value: m.id, label: m.id })), { value: this.model, label: 'Model', cls: 'w-auto max-w-full' });
    modelSel.addEventListener('change', () => {
      this.model = modelSel.value;
      this.restore();
    });
    const items: HTMLElement[] = [];
    if (this.accounts.length > 1) items.push(accountSel);
    items.push(modelSel);
    if (this.hasWeb) {
      const w = chip({ on: this.web }, 'Web search');
      w.addEventListener('click', () => {
        this.web = !this.web;
        setChipOn(w, this.web);
      });
      items.push(w);
    }
    this.pickers.replaceChildren(...items);
  }

  private key() {
    return chatKey(this.account, this.model);
  }

  private restore() {
    this.messages = load(this.key());
    this.draw();
  }

  private reset() {
    this.messages = [];
    save(this.key(), this.messages);
    this.draw();
  }

  private draw(thinking = false) {
    this.log.replaceChildren(
      ...(this.messages.length === 0 && !thinking ? [sheetHint('Say something. Replies come from the model you chose, with no tools unless you turn on web search.')] : []),
      ...this.messages.map((m) => this.bubble(m)),
      ...(thinking ? [sheetHint('Thinking…')] : []),
    );
    this.cost.textContent = this.messages.length ? spentLine(this.messages) : '';
    this.log.parentElement?.scrollTo({ top: this.log.parentElement.scrollHeight });
  }

  private bubble(m: ChatMsg): HTMLElement {
    const b = el('div', m.role === 'user' ? 'ml-8 self-end rounded-lg bg-accent-bg px-3 py-2 text-md' : 'mr-8 rounded-lg bg-muted px-3 py-2 text-md');
    b.style.whiteSpace = 'pre-wrap';
    b.style.overflowWrap = 'anywhere';
    b.textContent = m.content;
    if (m.role === 'assistant' && (m.tools?.length || m.costUsd != null)) {
      const meta = el('div', 'mt-1 text-xs text-muted-foreground');
      meta.textContent = [m.tools?.length ? `used ${m.tools.join(', ')}` : '', m.costUsd != null ? `$${m.costUsd.toFixed(4)}` : ''].filter(Boolean).join(' · ');
      b.append(meta);
    }
    return b;
  }

  private async ask() {
    const text = this.input.value.trim();
    if (!text || this.busy) return;
    this.busy = true;
    this.send.disabled = true;
    this.input.value = '';
    this.messages = push(this.messages, { role: 'user', content: text });
    this.draw(true);
    const r = await callJson<Reply>(
      '/api/chat-direct',
      { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ account: this.account, model: this.model, messages: toWire(this.messages), web: this.web }) },
      this.unauthorized,
    );
    this.busy = false;
    this.send.disabled = false;
    if (this.gone) return;
    if (r.ok) {
      this.messages = push(this.messages, { role: 'assistant', content: r.data.text || '(no answer)', costUsd: r.data.costUsd, tokens: { in: r.data.tokensIn, out: r.data.tokensOut }, tools: r.data.tools.map((t) => t.name) });
    } else {
      // The message stays unsent: put it back to try again, and say why.
      this.messages = this.messages.slice(0, -1);
      this.input.value = text;
    }
    save(this.key(), this.messages);
    this.draw();
    if (!r.ok) this.log.append(sheetMsg(r.error, true));
  }
}

