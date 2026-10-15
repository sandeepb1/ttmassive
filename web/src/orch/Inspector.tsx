// The panel for the selected agent: edit it before a run, watch it during one. With nothing selected it holds
// the settings of the whole flow (budget, retries).

import { useState } from 'react';
import { Icon, ROLE_ICON, Field, Switch } from './bits';
import { ROLE_INFO, cacheHit, fmtTokens, modelShown, totalTokens } from './model';
import type { Engine, Flow, FlowNode, Models, NodeRun, Run } from './types';

interface Props {
  flow: Flow;
  node: FlowNode | null;
  run: Run | null;
  models: Models;
  editable: boolean;
  goal: string;
  onNode: (id: string, patch: Partial<FlowNode>) => void;
  onFlow: (patch: Partial<Flow>) => void;
  onDelete: (id: string) => void;
  onWritePrompt: (node: FlowNode) => Promise<string | null>;
  onPopOut: (node: FlowNode) => Promise<void>;
  onClose: () => void;
}

const num = (v: string) => (v.trim() === '' ? null : Math.max(0, Number(v) || 0));

export function Inspector(p: Props) {
  return (
    <>
      <div className="mc-panel-head">
        <span style={{ flex: 1 }}>{p.node ? p.node.name : 'Flow settings'}</span>
        <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Close" onClick={p.onClose}>
          <Icon name="close" />
        </button>
      </div>
      <div className="mc-scroll">{p.node ? <NodePanel {...p} node={p.node} /> : <FlowPanel {...p} />}</div>
    </>
  );
}

function FlowPanel({ flow, editable, onFlow }: Props) {
  const b = flow.budget;
  const setBudget = (patch: Partial<typeof b>) => onFlow({ budget: { ...b, ...patch } });
  const pct = (v: string, fallback: number) => Math.min(100, Math.max(1, Number(v) || fallback));
  return (
    <>
      <p className="mc-hint" style={{ margin: '0 0 1rem' }}>
        Select an agent to edit it. These limits protect your plan: a run stops, and asks you, when it reaches one.
      </p>
      <Field label="Claude: most this run may use" hint="Share of your 5-hour Claude window.">
        <input className="mc-field mc-mono" type="number" min={1} max={100} disabled={!editable} value={b.claudePct} onChange={(e) => setBudget({ claudePct: pct(e.target.value, b.claudePct) })} />
      </Field>
      <Field label="Codex: most this run may use" hint="Share of your 5-hour Codex window.">
        <input className="mc-field mc-mono" type="number" min={1} max={100} disabled={!editable} value={b.codexPct} onChange={(e) => setBudget({ codexPct: pct(e.target.value, b.codexPct) })} />
      </Field>
      <Field label="Stop line" hint="Stop whenever any window reaches this, so your own sessions are never left with nothing.">
        <input className="mc-field mc-mono" type="number" min={1} max={100} disabled={!editable} value={b.stopWindowPct} onChange={(e) => setBudget({ stopWindowPct: pct(e.target.value, b.stopWindowPct) })} />
      </Field>
      <Field label="Times a judge may send work back" hint="After this many, the run pauses and asks you.">
        <input className="mc-field mc-mono" type="number" min={0} max={5} disabled={!editable} value={flow.maxRetries} onChange={(e) => onFlow({ maxRetries: Math.min(5, Math.max(0, Math.round(Number(e.target.value) || 0))) })} />
      </Field>
      <div className="mc-section">
        <Switch
          on={!!flow.downshift}
          disabled={!editable}
          onChange={(v) => onFlow({ downshift: v })}
          label="Use cheaper models as my plan fills up"
          hint="An agent that has not started yet gets the next model down (Opus, then Sonnet, then Haiku; Codex the same) once the run is half way to a limit, and the cheapest from three quarters. Never a dearer one. An agent keeps the model it started on, so its prompt cache is not lost."
        />
      </div>
    </>
  );
}

function NodePanel({ flow, node, run, models, editable, goal, onNode, onDelete, onWritePrompt, onPopOut }: Props & { node: FlowNode }) {
  const nr: NodeRun | undefined = run?.nodes[node.id];
  const [writing, setWriting] = useState(false);
  const [opening, setOpening] = useState(false);
  const modelList = node.engine === 'claude' ? models.claude : models.codex;
  const options = modelList.includes(node.model) ? modelList : [node.model, ...modelList];
  const setEngine = (engine: Engine) => {
    const list = engine === 'claude' ? models.claude : models.codex;
    onNode(node.id, { engine, model: list[0] ?? node.model });
  };
  const write = async () => {
    setWriting(true);
    const text = await onWritePrompt(node);
    setWriting(false);
    if (text) onNode(node.id, { systemPrompt: text });
  };
  const feedback = run?.feedback[node.id];

  return (
    <>
      <div className="mc-section" style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', color: 'var(--mc-dim)' }}>
        <Icon name={ROLE_ICON[node.role]} /> <span>{ROLE_INFO[node.role].label}</span>
        <span style={{ color: 'var(--mc-faint)' }}>· {ROLE_INFO[node.role].blurb}</span>
      </div>

      {nr && <RunDetail nr={nr} node={node} feedback={feedback} />}

      {nr && node.role === 'builder' && (
        <div className="mc-section">
          <button
            type="button"
            className="mc-btn"
            disabled={!nr.session || nr.status === 'running' || opening}
            onClick={() => {
              setOpening(true);
              void onPopOut(node).finally(() => setOpening(false));
            }}
          >
            <Icon name="shell" /> {opening ? 'Opening…' : 'Open in a live session'}
          </button>
          <p className="mc-hint">
            {nr.session
              ? 'Carry on with this builder’s conversation yourself, in its own copy of the project. The run does not wait for you; if it sends this builder work again, that starts a fresh agent.'
              : nr.status === 'running'
                ? 'Available once it has finished its turn.'
                : 'Available once it has finished a turn.'}
          </p>
        </div>
      )}

      <Field label="Name">
        <input className="mc-field" disabled={!editable} value={node.name} maxLength={60} onChange={(e) => onNode(node.id, { name: e.target.value })} />
      </Field>
      <div className="mc-grid2">
        <Field label="Engine">
          <select className="mc-field" disabled={!editable} value={node.engine} onChange={(e) => setEngine(e.target.value as Engine)}>
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
          </select>
        </Field>
        <Field label="Model">
          <select className="mc-field mc-mono" disabled={!editable} value={node.model} onChange={(e) => onNode(node.id, { model: e.target.value })}>
            {options.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </Field>
      </div>

      <Field label="Instructions" hint="Sent the same way on every turn, so the prompt cache holds. The goal is given separately.">
        <textarea className="mc-field" rows={7} disabled={!editable} value={node.systemPrompt} maxLength={16000} onChange={(e) => onNode(node.id, { systemPrompt: e.target.value })} />
        {editable && (
          <button type="button" className="mc-btn mc-btn-sm mc-q-use" disabled={writing || !goal.trim()} onClick={write} title={goal.trim() ? 'Sonnet writes them' : 'Needs a goal: start a run first or type one in the new-run box'}>
            <Icon name="wand" /> {writing ? 'Writing…' : 'Write with Sonnet'}
          </button>
        )}
      </Field>

      <div className="mc-section">
        <Switch on={node.checkpoint} disabled={!editable} onChange={(v) => onNode(node.id, { checkpoint: v })} label="Ask me before it starts" hint={node.role === 'builder' ? 'It changes files, in its own copy of the project.' : undefined} />
        <Switch on={!!node.keepModel} disabled={!editable} onChange={(v) => onNode(node.id, { keepModel: v })} label="Always use this model" hint="Never switch it to a cheaper one. Only matters when the flow uses cheaper models as the plan fills up." />
        {flow.edges.some((e) => e.kind === 'retry' && e.to === node.id) && <p className="mc-hint">A judge can send work back to this agent.</p>}
      </div>
      <Field label="Token cap" hint="Pause the run if this agent uses more than this. Cache reads do not count. Leave empty for no cap.">
        <input className="mc-field mc-mono" type="number" min={0} placeholder="No cap" disabled={!editable} value={node.tokenCap ?? ''} onChange={(e) => onNode(node.id, { tokenCap: num(e.target.value) })} />
      </Field>

      {editable && (
        <button type="button" className="mc-btn mc-btn-danger" onClick={() => onDelete(node.id)}>
          <Icon name="trash" /> Remove agent
        </button>
      )}
    </>
  );
}

function RunDetail({ nr, node, feedback }: { nr: NodeRun; node: FlowNode; feedback?: string }) {
  const t = nr.tokens;
  const hit = cacheHit(t);
  return (
    <div className="mc-section">
      <label className="mc-label">This run</label>
      <dl className="mc-kv">
        <dt>Status</dt>
        <dd>{nr.status}</dd>
        <dt>Turns</dt>
        <dd>{nr.attempts}</dd>
        <dt>Model</dt>
        <dd>{modelShown(node, nr).label}</dd>
        <dt>Fresh input</dt>
        <dd>{fmtTokens(t.input + t.cacheWrite)}</dd>
        <dt>Output</dt>
        <dd>{fmtTokens(t.output)}</dd>
        <dt>Read from cache</dt>
        <dd>
          {fmtTokens(t.cacheRead)}
          {hit === null ? '' : ` · ${Math.round(hit * 100)}%`}
        </dd>
        <dt>Total</dt>
        <dd>{fmtTokens(totalTokens(t))}</dd>
        {(nr.cacheBreaks ?? 0) > 0 && (
          <>
            <dt>Cache breaks</dt>
            <dd style={{ color: 'var(--mc-bad)' }}>{nr.cacheBreaks}</dd>
          </>
        )}
      </dl>
      {nr.error && <p className="mc-error">{nr.error}</p>}
      {feedback && (
        <>
          <label className="mc-label" style={{ marginTop: '0.75rem' }}>Judge’s feedback to fix</label>
          <pre className="mc-pre">{feedback}</pre>
        </>
      )}
      {nr.transcript.length > 0 && (
        <>
          <label className="mc-label" style={{ marginTop: '0.75rem' }}>Activity</label>
          <ul className="mc-log">
            {nr.transcript.slice(-12).map((l, i) => (
              <li key={`${i}-${l}`} title={l}>
                {l}
              </li>
            ))}
          </ul>
        </>
      )}
      {nr.output != null && (
        <>
          <label className="mc-label" style={{ marginTop: '0.75rem' }}>Handed on</label>
          <pre className="mc-pre">{JSON.stringify(nr.output, null, 2)}</pre>
        </>
      )}
    </div>
  );
}
