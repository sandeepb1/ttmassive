// The panel for the selected agent: edit it before a run, watch it during one. With nothing selected it holds
// the settings: the plan limits (the same for every flow, changeable any time) and this flow's own choices.

import { useState } from 'react';
import { Icon, iconFor, Field, Switch } from './bits';
import { CAPABILITY_SHORT, DIAL_TEXT, ON_CAP_TEXT, OUTPUT_TEXT, ROUTING_TEXT, nextRung, providerOf, providerPatch, providersFor, rungFromKey, rungKey, withRoutingChange, asks, cacheHit, canCallDirectly, consistent, fmtTokens, modelKey, modelPatch, modelShown, openRouterChoices, priceNote, roleBlurb, roleLabel, totalTokens, usdLabel, writes } from './model';
import { DEFAULT_DIAL, DEFAULT_ITERATIONS, EFFORTS, type Answerer, type Budget, type Capability, type Engine, type Flow, type FlowEdge, type FlowNode, type Models, type NodeRun, type OnCap, type Output, type Routing, type Rung, type Run } from './types';

interface Props {
  flow: Flow;
  node: FlowNode | null;
  /** A send-back edge (or any edge) selected instead of an agent. */
  edge: FlowEdge | null;
  run: Run | null;
  models: Models;
  editable: boolean;
  goal: string;
  limits: Budget;
  onLimits: (b: Budget) => void;
  onNode: (id: string, patch: Partial<FlowNode>) => void;
  onEdge: (id: string, patch: Partial<FlowEdge>) => void;
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
        <span style={{ flex: 1 }}>{p.node ? p.node.name : p.edge ? (p.edge.kind === 'retry' ? 'Send back' : 'Connection') : 'Settings'}</span>
        <button type="button" className="mc-btn mc-btn-quiet mc-btn-icon" aria-label="Close" onClick={p.onClose}>
          <Icon name="close" />
        </button>
      </div>
      <div className="mc-scroll">{p.node ? <NodePanel {...p} node={p.node} /> : p.edge ? <EdgePanel {...p} edge={p.edge} /> : <FlowPanel {...p} />}</div>
    </>
  );
}

function FlowPanel({ flow, run, editable, limits, onLimits, onFlow }: Props) {
  const b = limits;
  const pct = (v: string, fallback: number) => Math.min(100, Math.max(1, Number(v) || fallback));
  return (
    <>
      <p className="mc-hint" style={{ margin: '0 0 1rem' }}>
        Select an agent to edit it.
      </p>

      <div className="mc-group">
        <h3 className="mc-group-title">Plan limits</h3>
        <p className="mc-hint">
          The same for every flow. They protect your plan: a run stops, and asks you, when it reaches one. You can change them at any time, and a run that is going
          uses the new numbers the next time it checks.
        </p>
        <Field label="Most one run may use of Claude" hint="Share of your 5-hour Claude window.">
          <input className="mc-field mc-mono" type="number" min={1} max={100} value={b.claudePct} onChange={(e) => onLimits({ ...b, claudePct: pct(e.target.value, b.claudePct) })} />
        </Field>
        <Field label="Most one run may use of Codex" hint="Share of your 5-hour Codex window.">
          <input className="mc-field mc-mono" type="number" min={1} max={100} value={b.codexPct} onChange={(e) => onLimits({ ...b, codexPct: pct(e.target.value, b.codexPct) })} />
        </Field>
        <Field label="Stop everything at" hint="Stop whenever any window reaches this, so your own sessions are never left with nothing.">
          <input className="mc-field mc-mono" type="number" min={1} max={100} value={b.stopWindowPct} onChange={(e) => onLimits({ ...b, stopWindowPct: pct(e.target.value, b.stopWindowPct) })} />
        </Field>
        <Field label="Most one run may spend on API models" hint="Dollars, for agents on OpenRouter or another API account. The run pauses, and asks you, when it reaches this; an OpenRouter agent that would go past it mid-turn is stopped. Leave empty for no cap. Agents on your own plan are not counted.">
          <input
            className="mc-field mc-mono"
            type="number"
            min={0}
            step={0.5}
            placeholder="No cap"
            value={b.runUsd ?? ''}
            onChange={(e) => onLimits({ ...b, runUsd: Number(e.target.value) > 0 ? Number(e.target.value) : null })}
          />
        </Field>
        <Field label="Most one run may use on web access" hint="Dollars, at each web account's price per credit. The run pauses, and asks you, when it reaches this; an agent that would go past it is told to work from what it has. Leave empty for no cap.">
          <input
            className="mc-field mc-mono"
            type="number"
            min={0}
            step={0.05}
            placeholder="No cap"
            value={b.runWebUsd ?? ''}
            onChange={(e) => onLimits({ ...b, runWebUsd: Number(e.target.value) > 0 ? Number(e.target.value) : null })}
          />
        </Field>
        {run?.waived && <p className="mc-hint">You chose to carry on past the limits for this run, so it is not being stopped by them.</p>}
      </div>

      <div className="mc-group">
        <h3 className="mc-group-title">This flow</h3>
        {!editable && <p className="mc-hint">Viewing an execution: these were fixed when it started. Go back to the design to change them.</p>}
        <Field label="Times a judge may send work back" hint="After this many, the run pauses and asks you.">
          <input className="mc-field mc-mono" type="number" min={0} max={5} disabled={!editable} value={flow.maxRetries} onChange={(e) => onFlow({ maxRetries: Math.min(5, Math.max(0, Math.round(Number(e.target.value) || 0))) })} />
        </Field>
        {flow.nodes.some((n) => (n.routing ?? 'pinned') !== 'pinned') && (
          <Field label="Cost or quality" hint="Where agents on a ladder start: toward the cheapest rung, or the strongest. They still move up when work is sent back.">
            <input type="range" min={0} max={4} step={1} disabled={!editable} value={flow.dial ?? DEFAULT_DIAL} aria-label="Cost or quality" onChange={(e) => onFlow({ dial: Number(e.target.value) })} />
            <div className="mc-hint">{DIAL_TEXT[flow.dial ?? DEFAULT_DIAL]}</div>
          </Field>
        )}
        <div className="mc-section">
          <Switch
            on={!!flow.downshift}
            disabled={!editable}
            onChange={(v) => onFlow({ downshift: v })}
            label="Use cheaper models as my plan fills up"
            hint="An agent that has not started yet gets the next model down (Opus, then Sonnet, then Haiku; Codex the same) once the run is half way to a limit, and the cheapest from three quarters. Never a dearer one. An agent keeps the model it started on, so its prompt cache is not lost."
          />
        </div>
      </div>
    </>
  );
}

/** One connection: what it hands over, or, for a send-back, how often it may be used and what happens then. */
function EdgePanel({ flow, edge, run, editable, onEdge }: Props & { edge: FlowEdge }) {
  const from = flow.nodes.find((n) => n.id === edge.from);
  const to = flow.nodes.find((n) => n.id === edge.to);
  if (edge.kind !== 'retry') {
    return (
      <p className="mc-hint">
        {from?.name ?? 'An agent'} hands its output to {to?.name ?? 'the next agent'}. Nothing to set here.
      </p>
    );
  }
  const max = edge.maxIterations ?? DEFAULT_ITERATIONS;
  const used = run?.iterations?.[edge.id] ?? 0;
  return (
    <>
      <p className="mc-hint" style={{ margin: '0 0 1rem' }}>
        {from?.name ?? 'An agent'} can send work back to {to?.name ?? 'an earlier agent'}, which then runs again, and everything between them with it.
      </p>
      <Field label="Most times it may be used" hint={`In one run. The flow also has its own limit on all send-backs together (${flow.maxRetries}).`}>
        <input
          className="mc-field mc-mono"
          type="number"
          min={1}
          max={10}
          disabled={!editable}
          value={max}
          onChange={(e) => onEdge(edge.id, { maxIterations: Math.min(10, Math.max(1, Math.round(Number(e.target.value) || 1))) })}
        />
      </Field>
      <Field label="When it has been used that many times">
        <select className="mc-field" disabled={!editable} value={edge.onCap ?? 'pause'} onChange={(e) => onEdge(edge.id, { onCap: e.target.value as OnCap })}>
          {(Object.keys(ON_CAP_TEXT) as OnCap[]).map((k) => (
            <option key={k} value={k}>
              {ON_CAP_TEXT[k]}
            </option>
          ))}
        </select>
      </Field>
      {from && asks(from) && (
        <Field label="Who answers its questions" hint="The agent it asks about the work either gets your answers, or has to answer them itself.">
          <select className="mc-field" disabled={!editable} value={edge.answeredBy ?? 'user'} onChange={(e) => onEdge(edge.id, { answeredBy: e.target.value as Answerer })}>
            <option value="user">You: the run stops and asks</option>
            <option value="agent">{to?.name ?? 'The agent it goes back to'}: it answers in its next turn</option>
          </select>
        </Field>
      )}
      {run && (
        <p className="mc-hint">
          Used {used} of {max} {max === 1 ? 'time' : 'times'} in this run.
        </p>
      )}
      {!editable && <p className="mc-hint">These were fixed when the run started. Go back to the design to change them.</p>}
    </>
  );
}

/** Every model an agent on a ladder may use, in the groups the model picker has. */
function RungModelOptions({ models, current }: { models: Models; current: string }) {
  const known = new Set([...models.claude.map((m) => `claude:${m}`), ...models.codex.map((m) => `codex:${m}`), ...models.openrouter.filter((m) => !m.direct).map((m) => `or:${m.account}:${m.id}`)]);
  return (
    <>
      {!known.has(current) && <option value={current}>{current.replace(/^[a-z]+:/, '')} (not on offer)</option>}
      <optgroup label="Your Claude plan">
        {models.claude.map((m) => (
          <option key={m} value={`claude:${m}`}>
            {m}
          </option>
        ))}
      </optgroup>
      {models.codex.length > 0 && (
        <optgroup label="Your Codex plan">
          {models.codex.map((m) => (
            <option key={m} value={`codex:${m}`}>
              {m}
            </option>
          ))}
        </optgroup>
      )}
      {[...new Set(models.openrouter.filter((m) => !m.direct).map((m) => m.account))].map((acct) => (
          <optgroup key={acct} label={`OpenRouter · ${models.openrouter.find((m) => m.account === acct)!.accountName}`}>
            {models.openrouter
              .filter((m) => !m.direct && m.account === acct)
              .map((m) => (
                <option key={m.id} value={`or:${m.account}:${m.id}`}>
                  {m.id}
                </option>
              ))}
          </optgroup>
        ))}
    </>
  );
}

/** How an agent's model is chosen each turn, and the ladder it moves along. */
function RoutingEditor({ node, models, editable, onNode }: { node: FlowNode; models: Models; editable: boolean; onNode: (id: string, patch: Partial<FlowNode>) => void }) {
  const routing: Routing = node.routing ?? 'pinned';
  const ladder = node.ladder ?? [];
  const setLadder = (next: Rung[]) => onNode(node.id, { ladder: next });
  const setRung = (i: number, patch: Partial<Rung>) => setLadder(ladder.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const move = (i: number, to: number) => {
    if (to < 0 || to >= ladder.length) return;
    const next = [...ladder];
    next.splice(to, 0, next.splice(i, 1)[0]!);
    setLadder(next);
  };
  return (
    <>
      <Field label="Model selection" hint={routing === 'pinned' ? 'Instead of one model, give it a ladder of models and efforts, each on any account you have, that it moves along as the run goes: higher after work is sent back, lower as your plan fills up. A small model can choose the rung each turn.' : undefined}>
        <select className="mc-field" disabled={!editable || !!node.direct} value={routing} onChange={(e) => onNode(node.id, withRoutingChange(node, e.target.value as Routing, models))}>
          {(Object.keys(ROUTING_TEXT) as Routing[]).map((r) => (
            <option key={r} value={r}>
              {ROUTING_TEXT[r]}
            </option>
          ))}
        </select>
      </Field>
      {routing === 'pinned' ? (
        <Field label="Effort" hint="How hard it thinks. Left alone, the agent’s own default.">
          <select className="mc-field" disabled={!editable} value={node.effort ?? ''} onChange={(e) => onNode(node.id, { effort: e.target.value || null })}>
            <option value="">Default</option>
            {EFFORTS.map((x) => (
              <option key={x} value={x}>
                {x}
              </option>
            ))}
          </select>
        </Field>
      ) : (
        <div className="mc-section">
          <label className="mc-label">Ladder, cheapest and lightest first</label>
          {ladder.map((r, i) => (
            <div key={i} className="mc-rung">
              <span className="mc-rung-n">{i + 1}</span>
              <select className="mc-field mc-mono" disabled={!editable} value={rungKey(r)} aria-label={`Model for rung ${i + 1}`} onChange={(e) => setRung(i, rungFromKey(e.target.value, models, r.effort))}>
                <RungModelOptions models={models} current={rungKey(r)} />
              </select>
              <select className="mc-field" disabled={!editable} value={r.effort ?? ''} aria-label={`Effort for rung ${i + 1}`} onChange={(e) => setRung(i, { effort: e.target.value || null })}>
                <option value="">default effort</option>
                {EFFORTS.map((x) => (
                  <option key={x} value={x}>
                    {x}
                  </option>
                ))}
              </select>
              {editable && (
                <span className="mc-rung-tools">
                  <button type="button" className="mc-btn mc-btn-quiet mc-btn-sm" aria-label={`Move rung ${i + 1} up`} disabled={i === 0} onClick={() => move(i, i - 1)}>
                    <Icon name="chevron-up" />
                  </button>
                  <button type="button" className="mc-btn mc-btn-quiet mc-btn-sm" aria-label={`Move rung ${i + 1} down`} disabled={i === ladder.length - 1} onClick={() => move(i, i + 1)}>
                    <Icon name="chevron-down" />
                  </button>
                  <button type="button" className="mc-btn mc-btn-quiet mc-btn-sm" aria-label={`Remove rung ${i + 1}`} disabled={ladder.length <= 1} onClick={() => setLadder(ladder.filter((_, j) => j !== i))}>
                    <Icon name="close" />
                  </button>
                </span>
              )}
            </div>
          ))}
          {editable && ladder.length < 8 && (
            <button type="button" className="mc-btn mc-btn-sm" onClick={() => setLadder([...ladder, nextRung({ ...node, ladder }, models)])}>
              <Icon name="plus" /> Add a rung
            </button>
          )}
          <p className="mc-hint">
            {routing === 'auto'
              ? 'A small model picks a rung each time the agent starts, and says why. If it cannot, the rules choose. The flow’s cost-to-quality dial is part of what it is told.'
              : 'The flow’s dial sets where it starts. Each time its work is sent back, or its last try failed, it moves up a rung; as your plan fills up it moves down.'}{' '}
            Moving to another model costs the new one a fresh prompt cache for that turn.
          </p>
        </div>
      )}
    </>
  );
}

/** Where an agent runs, which CLI runs it, and which model. */
function ModelFields({ node, models, editable, options, onNode, setEngine }: { node: FlowNode; models: Models; editable: boolean; options: string[]; onNode: (id: string, patch: Partial<FlowNode>) => void; setEngine: (e: Engine) => void }) {
  const providers = providersFor(node, models);
  const here = providerOf(node);
  const account = node.account ? here.slice(3) : null;
  const mine = models.openrouter.filter((m) => m.account === account);
  const known = mine.some((m) => m.id === node.model);
  const offerOnly = models.openrouter.length === 0;
  return (
    <>
      <Field
        label="Runs on"
        hint={
          offerOnly
            ? 'Your Claude or Codex plan. To use OpenRouter, add an account and pin some models in Settings, under Account; turn on "Use in orchestrator flows".'
            : here === 'plan'
              ? 'Counts against your plan. Pick an account to pay per token instead.'
              : 'Billed per token by the account, not your plan.'
        }
      >
        <select className="mc-field" disabled={!editable || offerOnly} value={here} aria-label="Runs on" onChange={(e) => onNode(node.id, providerPatch(e.target.value, node, models))}>
          {providers.map((p) => (
            <option key={p.key} value={p.key} disabled={p.direct && !canCallDirectly(node) && p.key !== here}>
              {p.label}
              {p.direct && !canCallDirectly(node) && p.key !== here ? ' (this role needs tools)' : ''}
            </option>
          ))}
        </select>
      </Field>
      <div className="mc-grid2">
        <Field label={account ? 'Runs through' : 'Engine'}>
          <select className="mc-field" disabled={!editable || !!node.direct} value={node.direct ? 'direct' : node.engine} onChange={(e) => setEngine(e.target.value as Engine)}>
            {node.direct && <option value="direct">Direct call</option>}
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
          </select>
        </Field>
        <Field label="Model">
          <select className="mc-field mc-mono" disabled={!editable} value={account ? node.model : `plan:${node.model}`} onChange={(e) => onNode(node.id, account ? modelPatch(`or:${account}:${e.target.value}`, models) : modelPatch(e.target.value, models))}>
            {account ? (
              <>
                {!known && <option value={node.model}>{node.model} (not pinned)</option>}
                {mine.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.id}
                  </option>
                ))}
              </>
            ) : (
              options.map((m) => (
                <option key={m} value={`plan:${m}`}>
                  {m}
                </option>
              ))
            )}
          </select>
        </Field>
      </div>
    </>
  );
}

function NodePanel({ flow, node, run, models, editable, goal, onNode, onDelete, onWritePrompt, onPopOut }: Props & { node: FlowNode }) {
  const nr: NodeRun | undefined = run?.nodes[node.id];
  const [writing, setWriting] = useState(false);
  const [opening, setOpening] = useState(false);
  const modelList = node.engine === 'claude' ? models.claude : models.codex;
  // A model this node already has stays in the list even when it is no longer offered, so it is never changed silently.
  const options = node.account || modelList.includes(node.model) ? modelList : [node.model, ...modelList];
  const choices = openRouterChoices(node, models);
  const chosen = models.openrouter.find((m) => m.account === node.account && m.id === node.model);
  const setEngine = (engine: Engine) => {
    // An agent on OpenRouter keeps its model and account: only the CLI that runs it changes.
    if (node.account) return onNode(node.id, { engine });
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
        <Icon name={iconFor(node)} /> <span>{roleLabel(node)}</span>
        <span style={{ color: 'var(--mc-faint)' }}>· {roleBlurb(node)}</span>
      </div>
      {node.custom && editable && (
        <div className="mc-section">
          <div className="mc-grid2">
            <Field label="Role name">
              <input className="mc-field" value={node.custom.label} maxLength={40} onChange={(e) => onNode(node.id, { custom: { ...node.custom!, label: e.target.value } })} />
            </Field>
            <Field label="May">
              <select
                className="mc-field"
                value={node.custom.capability}
                onChange={(e) => {
                  const capability = e.target.value as Capability;
                  onNode(node.id, { custom: { ...node.custom!, capability, output: consistent(capability, node.custom!.output) }, checkpoint: capability === 'write' ? true : node.checkpoint });
                }}
              >
                {(Object.keys(CAPABILITY_SHORT) as Capability[]).map((c) => (
                  <option key={c} value={c}>
                    {CAPABILITY_SHORT[c]}
                  </option>
                ))}
              </select>
            </Field>
          </div>
          <Field label="Hands back" hint={node.custom.capability === 'write' ? 'An agent that changes files reports the change.' : 'A verdict lets it send work back to an earlier agent.'}>
            <select
              className="mc-field"
              value={node.custom.output}
              disabled={node.custom.capability === 'write'}
              onChange={(e) => onNode(node.id, { custom: { ...node.custom!, output: e.target.value as Output } })}
            >
              {(Object.keys(OUTPUT_TEXT) as Output[])
                .filter((o) => (node.custom!.capability === 'write' ? o === 'change' : o !== 'change'))
                .map((o) => (
                  <option key={o} value={o}>
                    {OUTPUT_TEXT[o]}
                  </option>
                ))}
            </select>
          </Field>
        </div>
      )}

      {nr && <RunDetail nr={nr} node={node} feedback={feedback} />}

      {nr && writes(node) && (
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
      <ModelFields node={node} models={models} editable={editable} options={options} onNode={onNode} setEngine={setEngine} />
      <RoutingEditor node={node} models={models} editable={editable} onNode={onNode} />
      {node.account && node.direct && (
        <p className="mc-hint">
          Called directly: no tools and no files, so it works only from the brief and what earlier agents hand on. {chosen ? `${priceNote(chosen)}. ` : ''}
          Its cost is counted toward this run’s dollar cap when the account has prices.
        </p>
      )}
      {node.account && !node.direct && (
        <p className="mc-hint">
          Billed by OpenRouter, not your plan. {chosen ? `${priceNote(chosen)}. ` : ''}
          {node.engine === 'claude' ? 'Claude Code' : 'Codex'} runs it through ttm, and its cost is counted toward this run’s dollar cap.
        </p>
      )}

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
        {(models.web || node.web) && (
          <Switch
            on={!!node.web}
            disabled={!editable}
            onChange={(v) => onNode(node.id, { web: v })}
            label="Web search and page fetching"
            hint={models.web ? 'Through your Firecrawl account. Each search or page uses credits, counted toward this run’s cap. Agents that look things up have it on by default.' : 'There is no Firecrawl account to give it with: add one in Settings, under Account.'}
          />
        )}
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
        <dd>{modelShown(node, nr).label}{nr.effortUsed ? ` · ${nr.effortUsed} effort` : ''}</dd>
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
        {node.account ? (
          <>
            <dt>Cost</dt>
            <dd>{usdLabel(nr.costUsd ?? 0)} {node.direct ? 'by price' : 'on OpenRouter'}</dd>
          </>
        ) : (nr.costUsd ?? 0) > 0 ? (
          <>
            <dt>Cost</dt>
            <dd title="Claude Code's estimate at API prices. Your plan was not charged this.">≈ {usdLabel(nr.costUsd ?? 0)} at API prices (not billed)</dd>
          </>
        ) : null}
        {(nr.credits ?? 0) > 0 && (
          <>
            <dt>Web</dt>
            <dd>
              {nr.credits} credit{nr.credits === 1 ? '' : 's'}
              {(nr.webUsd ?? 0) > 0 ? ` · ${usdLabel(nr.webUsd ?? 0)}` : ''}
            </dd>
          </>
        )}
        {(nr.cacheBreaks ?? 0) > 0 && (
          <>
            <dt>Cache breaks</dt>
            <dd style={{ color: 'var(--mc-bad)' }}>{nr.cacheBreaks}</dd>
          </>
        )}
      </dl>
      {nr.routeReason && <p className="mc-hint">{nr.routeReason}</p>}
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
