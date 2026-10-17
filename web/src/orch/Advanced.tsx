// An agent's advanced settings, under four headings: how it is held in check, how it is kept going, what it is shown, and
// what it may use. Everything starts off, so an agent that never opens this behaves as it always did. What each one does is
// enforced by the runner, not asked of the model.

import { useState, type ReactNode } from 'react';
import { Field, Switch } from './bits';
import { ADV, advancedCount, fallbackChoices, feeders, formatDuration, parseDuration, parseToolNames, parseUsdCap, parseWhole, rungKey, withAdvanced, capabilityOf } from './model';
import type { Advanced, Flow, FlowNode, Models } from './types';

interface Props {
  flow: Flow;
  node: FlowNode;
  models: Models;
  editable: boolean;
  onNode: (id: string, patch: Partial<FlowNode>) => void;
}

/** A setting typed into a box. What is typed is kept as it is while it is being typed, and checked each time; a value that is
 *  understood is saved at once and one that is not is marked. Leaving the box shows the saved value tidily. */
function Typed<T>({ label, hint, current, show, parse, onSet, placeholder, disabled, example }: {
  label: string;
  hint: ReactNode;
  current: T | null | undefined;
  show: (v: T) => string;
  parse: (text: string) => T | null | 'bad';
  onSet: (v: T | null) => void;
  placeholder: string;
  disabled: boolean;
  example?: string;
}) {
  const [text, setText] = useState(current == null ? '' : show(current));
  const bad = parse(text) === 'bad';
  return (
    <Field label={label} hint={typeof hint === 'string' ? hint : undefined}>
      <input
        className="mc-field mc-mono"
        value={text}
        placeholder={placeholder}
        disabled={disabled}
        aria-label={label}
        aria-invalid={bad || undefined}
        onChange={(e) => {
          setText(e.target.value);
          const p = parse(e.target.value);
          if (p !== 'bad') onSet(p);
        }}
        onBlur={() => !bad && setText(current == null ? '' : show(current))}
      />
      {bad && <p className="mc-error" role="alert">{example ?? 'That is not a value it can use.'}</p>}
      {typeof hint !== 'string' && hint}
    </Field>
  );
}

export function AdvancedSection({ flow, node, models, editable, onNode }: Props) {
  const a: Advanced = node.advanced ?? {};
  const set = (patch: Partial<Advanced>) => onNode(node.id, { advanced: withAdvanced(node.advanced, patch) });
  const count = advancedCount(node.advanced);
  const fed = feeders(flow, node.id);
  const writes = capabilityOf(node) === 'write';
  const choices = fallbackChoices(models);
  const off = !editable;
  const retries = a.autoRetries ?? 0;

  return (
    <details className="mc-adv" open={count > 0}>
      <summary>
        Advanced settings{count > 0 && <span className="mc-pill" data-tone="live">{count} set</span>}
      </summary>
      {node.direct && <p className="mc-hint">This agent calls its model directly, with no tools, so only its time limit, retries and what it is shown apply.</p>}

      <h4 className="mc-adv-head">Limits</h4>
      <Typed
        key={`time-${node.id}`}
        label="Time limit"
        hint="Stops a turn that takes longer than this and counts it as failed. Type 90s, 10m or 2h. Leave empty for no limit."
        current={a.timeLimitSecs}
        show={formatDuration}
        parse={parseDuration}
        onSet={(v) => set({ timeLimitSecs: v })}
        placeholder="No limit"
        example="Try 90s, 10m or 2h, up to 24 hours."
        disabled={off}
      />
      <Typed
        key={`cost-${node.id}`}
        label="Cost cap"
        hint="Most dollars it may spend in one run, when it is paid for per token: an OpenRouter agent, or a Claude Code agent by Claude Code's own estimate. Leave empty for no cap."
        current={a.costCapUsd}
        show={(v) => String(v)}
        parse={parseUsdCap}
        onSet={(v) => set({ costCapUsd: v })}
        placeholder="No cap"
        example="A dollar amount above 0, like 0.50."
        disabled={off || !!node.direct}
      />
      <Typed
        key={`calls-${node.id}`}
        label="Most tool calls"
        hint="Stops a turn that makes more tool calls than this (commands, file reads, searches) and counts it as failed. Leave empty for no limit."
        current={a.maxToolCalls}
        show={(v) => String(v)}
        parse={(t) => parseWhole(t, 1, ADV.calls)}
        onSet={(v) => set({ maxToolCalls: v })}
        placeholder="No limit"
        example={`A whole number from 1 to ${ADV.calls}.`}
        disabled={off || !!node.direct}
      />

      <h4 className="mc-adv-head">Resilience</h4>
      <Field label="Try again by itself" hint="When a turn fails, run it again this many times before the run stops and asks you. Each try is counted as an attempt.">
        <select className="mc-field" aria-label="Try again by itself" disabled={off} value={retries} onChange={(e) => set({ autoRetries: Number(e.target.value) })}>
          {['Never: stop and ask', 'Once', 'Twice', 'Three times'].map((t, i) => (
            <option key={t} value={i}>
              {t}
            </option>
          ))}
        </select>
      </Field>
      {retries > 0 && !node.direct && (
        <Field label="Run the retry on" hint="A different model for the try after a failure, such as one that is not overloaded. The agent goes back to its own model afterwards.">
          <select
            className="mc-field"
            aria-label="Run the retry on"
            disabled={off}
            value={a.fallback ? rungKey(a.fallback) : ''}
            onChange={(e) => set({ fallback: choices.find((c) => c.key === e.target.value)?.rung ?? null })}
          >
            <option value="">The same model</option>
            {choices.map((c) => (
              <option key={c.key} value={c.key}>
                {c.label}
              </option>
            ))}
          </select>
        </Field>
      )}

      <h4 className="mc-adv-head">Context</h4>
      {fed.length > 1 ? (
        <Field label="Results it is shown" hint="Which of the agents that feed it it is given the results of. All of them is the default.">
          <div className="mc-checks">
            {fed.map((f) => {
              const shown = a.inputs ? a.inputs.includes(f.id) : true;
              return (
                <label key={f.id} className="mc-check">
                  <input
                    type="checkbox"
                    checked={shown}
                    disabled={off}
                    onChange={(e) => {
                      const now = new Set(a.inputs ?? fed.map((x) => x.id));
                      if (e.target.checked) now.add(f.id);
                      else now.delete(f.id);
                      // Taking every one away would leave it nothing; ticking all of them is the same as the default.
                      if (now.size === 0) return;
                      set({ inputs: now.size === fed.length ? null : fed.filter((x) => now.has(x.id)).map((x) => x.id) });
                    }}
                  />
                  {f.name}
                </label>
              );
            })}
          </div>
        </Field>
      ) : (
        <p className="mc-hint">{fed.length === 0 ? 'Nothing feeds this agent, so it is shown only the brief.' : `Only ${fed[0]!.name} feeds this agent, so it is shown that.`}</p>
      )}
      <Typed
        key={`chars-${node.id}`}
        label="Cut each result to"
        hint="Characters of each result it is shown, so a long one does not fill its context. A note says where it was cut. Leave empty for the whole of it."
        current={a.inputChars}
        show={(v) => String(v)}
        parse={(t) => parseWhole(t, ADV.charsMin, ADV.charsMax)}
        onSet={(v) => set({ inputChars: v })}
        placeholder="The whole result"
        example={`A whole number from ${ADV.charsMin} to ${ADV.charsMax}.`}
        disabled={off}
      />
      <Switch on={!!a.freshOnRetry} disabled={off} onChange={(v) => set({ freshOnRetry: v })} label="Start fresh each time it runs" hint="Forget the conversation it had when work is sent back to it or it is tried again, and begin again from the brief and its inputs. Costs the cache, and avoids carrying a wrong turn forward." />

      <h4 className="mc-adv-head">Access</h4>
      {!writes && !node.direct && (
        <Switch on={!!a.shell} disabled={off} onChange={(v) => set({ shell: v })} label="Let it run commands" hint="In a sandbox that cannot write anywhere, as a judge's shell works, so it can run tests or git status. Claude Code agents only: Codex agents can already." />
      )}
      {!node.direct && (
        <Typed
          key={`deny-${node.id}`}
          label="Tools taken away"
          hint="By name, separated by commas, for Claude Code agents. For example WebFetch, or Bash(git push:*) to stop one command."
          current={a.denyTools?.length ? a.denyTools : null}
          show={(v) => v.join(', ')}
          parse={(t) => {
            const names = parseToolNames(t);
            return names === 'bad' ? 'bad' : names.length ? names : null;
          }}
          onSet={(v) => set({ denyTools: v ?? [] })}
          placeholder="None"
          example={`Tool names only, up to ${ADV.tools}.`}
          disabled={off}
        />
      )}
    </details>
  );
}
