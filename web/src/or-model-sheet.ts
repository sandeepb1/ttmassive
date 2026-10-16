// The model picker for a session that runs on OpenRouter. Claude's and Codex's own `/model` menus
// do not apply there: the model is whatever the session's lease says, so switching is a call to
// the server, and the CLI's next request goes to the new model. Only models the session's CLI can
// run are offered; the rest are listed and disabled.

import { callJson, getJson } from './api-json';
import {
  DRIVER_LABEL,
  contextLabel,
  priceLine,
  shortModel,
  spendLine,
  switchable,
  type Account,
  type Driver,
  type SessionLease,
} from './accounts-model';
import { Sheet } from './sheet';
import { row } from './ui/row';
import { sheetBlock, sheetHint, sheetLabel, sheetList, sheetMsg } from './ui/sheet-parts';

/** The session's lease, or null when it does not run on OpenRouter (or the server cannot say). */
export async function sessionLease(session: string, signedOut: () => void): Promise<SessionLease | null> {
  const r = await getJson<SessionLease[]>('/api/openrouter/leases', signedOut);
  return r.ok ? (r.data.find((l) => l.tag === session) ?? null) : null;
}

export class OrModelSheet extends Sheet {
  constructor(
    host: HTMLElement,
    session: string,
    private driver: Driver,
    private unauthorized: () => void,
    /** The model was switched; `message` says so. */
    private changed: (model: string, message: string) => void,
  ) {
    super(host, session, 'Model', unauthorized);
  }

  async open() {
    this.message('Loading…');
    this.show();
    const lease = await sessionLease(this.session, this.unauthorized);
    const accounts = await getJson<Account[]>('/api/accounts', this.unauthorized);
    if (this.gone) return;
    const account = accounts.ok ? accounts.data.find((a) => a.id === lease?.account) : undefined;
    if (!lease || !account) return this.message('Could not read this session’s OpenRouter account.', true);
    this.render(lease, account);
  }

  private render(lease: SessionLease, account: Account) {
    const list = sheetList();
    for (const { model, ok, current } of switchable(account, lease.model, this.driver)) {
      const need = this.driver === 'claude' ? 'codex' : 'claude';
      const bits = [priceLine(model), model.tools ? '' : 'no tools', ok ? '' : `needs ${DRIVER_LABEL[need]}`].filter(Boolean);
      const r = row(shortModel(model.id), { hint: bits.join(' · '), mark: current ? 'Current' : '', current });
      r.title = model.id;
      r.disabled = !ok;
      if (!ok) r.classList.add('opacity-50');
      else if (!current) r.addEventListener('click', () => void this.choose(model.id, r));
      list.append(r);
    }
    const spent = `${spendLine(lease.spent.costUsd, lease.capUsd)} spent on this session so far`;
    this.body.replaceChildren(
      sheetBlock(sheetLabel(account.name), sheetHint(spent)),
      list,
      sheetHint('Switching starts the prompt cache over, so the next reply costs more than usual. The agent may keep showing the old model name; the one marked Current is what answers.'),
    );
  }

  private async choose(model: string, btn: HTMLButtonElement) {
    if (this.busy) return;
    this.busy = true;
    btn.disabled = true;
    const r = await callJson<unknown>(
      `/api/openrouter/leases/${encodeURIComponent(this.session)}/model`,
      { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ model }) },
      this.unauthorized,
    );
    this.busy = false;
    // The server answers 204 with no body, which callJson reports as unreadable; the status says what happened.
    if (!r.ok && r.status !== 204) {
      btn.disabled = false;
      this.body.prepend(sheetMsg(r.error, true));
      return;
    }
    this.changed(model, `Now using ${shortModel(model)}. The next reply comes from it.`);
    this.close();
  }
}
