# Multi-agent roles for ttm: evidence and recommended role list

Researched 2026-10-15. Numbers and quotes below were read in the cited source unless listed under "Could not verify". Where a point is my own inference rather than a finding, it is marked "(inference)". Source numbers like [2] refer to "Evidence by source" at the end.

## 1. Summary

Nine roles cover the real flows. Four are core, five are situational. All six existing ids stay. Three are new (architect, debugger, reporter).

The evidence points one way. Splitting by function pays when the pieces are read-only and independent, or when one agent checks another's work with a signal from outside the model (tests, a diff in fresh context). It loses when several agents write at once or when each hand-off drops context [2][11][12][13]. So the list has many read roles, exactly one writer, and one checker that can run things.

| id | Display name | Stage | Capability | Result | Tier | Status |
|---|---|---|---|---|---|---|
| orchestrator | Orchestrator | plan | read | plan | strong | CORE |
| search | Search | gather | read | report | cheap | CORE |
| builder | Builder | build | write | change | balanced | CORE |
| judge | Judge | check | run checks | verdict | strong | CORE |
| researcher | Researcher | gather | read (files and web) | report | balanced | SITUATIONAL |
| analyst | Analyst | gather | read | report | strong | SITUATIONAL |
| architect | Architect | plan | read | plan | strong | SITUATIONAL |
| debugger | Debugger | gather | run checks | report | balanced | SITUATIONAL |
| reporter | Reporter | deliver | read | report | balanced | SITUATIONAL |

Why only nine (the brief guessed 10 to 14): each extra role costs tokens [2][6] and adds a hand-off where context is lost [13]. I found no source that supports a separate product manager, CEO, CTO, project manager, tester, designer or documenter agent for a one-person tool. Section 3 explains each rejection.

Three cautions that shape everything below:

1. Cost scales with agent count. Anthropic measured about 4x the tokens of chat for a single agent and about 15x for multi-agent [2]. Codex documents the same direction [6]. On a subscription with usage windows, every added agent must earn its place.
2. Do not run writers in parallel on one task. Cognition's argument, still maintained in 2026, is that parallel writers make conflicting implicit decisions [11][12]. Anthropic also names coding as a poor fit for multi-agent because it has few truly parallelizable tasks [2].
3. A judge that only reads is the weakest checker. Models do not reliably self-correct without external feedback [21], LLM judges favour their own style [22], and every gain I could quantify for verification came from running something (tests) [14][17]. Give the judge the ability to run checks, use a different model from the builder where possible, and cap retries.

## 2. Role catalogue

Default prompts contain no goal. The goal arrives per run, in the task text the orchestrator or user supplies. Prompts follow the structure Anthropic recommends for delegation: objective, output format, tool guidance, boundaries [2].

### Stage: plan

#### orchestrator (CORE)
- Purpose: turn the goal into a flow of agents, decide what runs in parallel, and set the retry limit.
- Capability: read. Result: plan. Tier: strong.
- Pays for itself when the subtasks cannot be predicted in advance. Anthropic defines orchestrator-workers as "a central LLM dynamically breaks down tasks, delegates them to worker LLMs, and synthesizes their results", suited to "complex tasks where you can't predict the subtasks needed" [1]. Magentic-One's Orchestrator "formulates a plan, maintains structured working memory of progress, directs tasks to other agents, restarts and resets upon stalling, and determines task completion" [16]. Removing its two ledgers cost 31% of performance on the GAIA validation set [16].
- Does not pay when the work is a known shape (use a template flow from section 4) or the diff fits in one sentence. Claude Code's own guide says "If you could describe the diff in one sentence, skip the plan" [5].
- Tier reason: it is the single point where a bad decomposition multiplies cost. Anthropic's best setup was a stronger lead with cheaper subagents [2].
- Default prompt: "You design the smallest flow of agents that can complete the task. Choose roles from the catalogue, say what each one must hand back, mark which steps are independent and can run in parallel, and set a retry limit for any check that can send work back. Use one agent when one is enough. Give every step an objective, the inputs it needs written out in full (the next agent sees nothing else), and a boundary on what it must not do. Do not do the work yourself and do not add a step whose output no later step uses."

#### architect (SITUATIONAL)
- Purpose: turn findings into a concrete, file-level implementation plan for the builder.
- Capability: read. Result: plan. Tier: strong.
- Pays when the change touches several files, the approach is uncertain, or the code is unfamiliar. Claude Code's guide says planning "is most useful when you're uncertain about the approach, when the change modifies multiple files, or when you're unfamiliar with the code being modified" [5]. Claude Code ships a read-only built-in `Plan` subagent for "Codebase research during plan mode" [3]. MetaGPT's ablation (Table 3) shows a measured gain from adding an Architect to Engineer plus Product Manager: human revisions fell from 6.5 to 4.0 and executability rose from 2.0 to 2.5, at $1.059 to $1.204 [14].
- Does not pay for small, clear changes. The same MetaGPT table shows cost rising with every role (see section 3) [14].
- Tier reason: a wrong plan is expensive to find later, and planning is short, so a strong model costs little here (inference).
- Default prompt: "You turn findings into an implementation plan for someone who will write the code and cannot ask you questions. List the files to change, the change in each, the order, the interfaces that must not change, and the exact checks that prove it works. Name risks and what you assumed. Do not write the code, and do not widen the scope beyond what the inputs ask for."

### Stage: gather

#### search (CORE)
- Purpose: find the files, symbols and facts that answer a narrow question and return locations, not file dumps.
- Capability: read. Result: report. Tier: cheap.
- Pays when it is fanned out wide and the answer is small. Claude Code's built-in `Explore` is read-only and used for "File discovery, code search, codebase exploration" [3]. Codex's built-in `explorer` is a "read-heavy codebase exploration agent" [6]. Subagent isolation keeps "intermediate tool calls and results" out of the parent, which "receives a concise summary, not every file the subagent read" [4]. Cognition says read-only subagents are the most common working multi-agent setup, but "mostly resemble tool calls rather than true multi-agent collaboration" [12].
- Does not pay if the lead already knows the file or the question is one grep.
- Tier reason: the job is mechanical and high volume. The Claude Code docs show an `Explore` override with `model: haiku` as the intended pattern [3].
- Default prompt: "You answer one narrow question about a codebase or a set of documents. Return a short list of findings, each with a file path and line range or a quoted fragment, and say plainly when something was not found. Stop as soon as the question is answered. Do not summarise whole files, do not propose changes, and do not search beyond the scope you were given."

#### researcher (SITUATIONAL)
- Purpose: gather and cross-check information from the web and documentation, with sources.
- Capability: read (files and web). Result: report. Tier: balanced.
- Pays when the answer is outside the repo (APIs, papers, standards) or needs several independent angles. Anthropic's research system used parallel search subagents and found multi-agent "excels" there [2]. Effort scaling in that post: simple fact-finding 1 agent with 3 to 10 tool calls; direct comparisons 2 to 4 subagents with 10 to 15 calls each; complex research more than 10 subagents [2].
- Does not pay for questions the codebase or one lookup answers. Anthropic's early failures included "spawning 50 subagents for simple queries" and "scouring the web endlessly for nonexistent sources", and picking SEO content farms over authoritative sources [2].
- Tier reason: judgement about source quality matters, but the volume is high (inference).
- Default prompt: "You research one question using primary sources: official documentation, the project's own repository, original papers. Return findings as claims, each with the source URL and the exact passage relied on. Say what you could not verify. Prefer fewer, better sources over many weak ones, and stop when the question is answered. Do not give recommendations, and do not state anything you did not see in a source."

#### analyst (SITUATIONAL)
- Purpose: weigh several reports against each other, resolve conflicts and rank options.
- Capability: read. Result: report. Tier: strong.
- Pays when two or more gatherers returned overlapping or conflicting findings, or a decision has trade-offs. It is the weakest-evidenced role: Anthropic's lead agent does this synthesis inside the orchestrator [2], and Magentic-One has no separate analyst [16]. Keep it because the user's existing flows use it and because a strong model reading only compact reports is cheap (inference).
- Does not pay with a single input report. Skip it and let the orchestrator or reporter synthesise.
- Tier reason: reasoning over inputs, no tool calls, so strong costs little.
- Default prompt: "You compare the reports you are given. Identify where they agree, where they conflict, and what is missing. For each decision the reports bear on, rank the options with the reason, and state your confidence. Quote the report each point came from. Do not do new research, do not invent facts that are not in the inputs, and do not write code."

#### debugger (SITUATIONAL)
- Purpose: reproduce a failure and narrow it to a root cause, with command output as evidence.
- Capability: run checks. Result: report. Tier: balanced.
- Pays for bug fixes, flaky tests and regressions. Claude Code's guide prescribes "write a failing test that reproduces the issue, then fix it" and "address the root cause, don't suppress the error" [5]. Agentless found that generating reproduction tests lifted SWE-bench Lite from 81 to 96 fixes (27.00% to 32.00%) at about $0.25 more per issue [17].
- Does not pay when the failure is already reproduced (a failing test or stack trace is in the task) or the task is a new feature.
- Tier reason: needs reasoning to form hypotheses, but the loop is bounded by command output (inference).
- Default prompt: "You find why something fails. Reproduce the failure first and record the exact command and output. Narrow it with the smallest experiments you can run, and report the root cause, the evidence for it (commands and output), and the files involved. If you cannot reproduce it, say so and what you tried. You can run read-only commands only. Do not edit files and do not propose a fix beyond one sentence."

### Stage: build

#### builder (CORE)
- Purpose: make the change in its own worktree and hand back a diff.
- Capability: write (own git worktree/branch). Result: change. Tier: balanced.
- Pays whenever the output is code or files. Keep exactly one writer per piece of work. Cognition: "Actions carry implicit decisions, and conflicting decisions carry bad results" [11]; in 2026 it still says parallel-writer swarms "don't see meaningful adoption" and that "unstructured swarms" are "mostly a distraction" [12]. Codex advises more care with parallel write-heavy work because of conflicts [6]. Claude Code does support worktree isolation per subagent and fan-out for disjoint migrations (`/batch`, 5 to 30 subagents, each in its own worktree) [3][5]. Use that only when the files are disjoint.
- Does not pay as several builders on overlapping files.
- Tier reason: writing is where quality shows. Balanced is enough when the plan is explicit; use strong for design-heavy changes (inference).
- Default prompt: "You implement the change described in your inputs, in your own worktree. Follow the plan and the existing patterns in the code, run the checks you were given, and fix failures you caused. Hand back a summary of what changed and why, the files touched, and the output of the checks you ran. Do not change anything outside the stated scope, do not skip or weaken tests to make them pass, and say clearly when you were unable to finish part of the task."

### Stage: check

#### judge (CORE)
- Purpose: decide whether the work meets the stated criteria, using evidence it gathers itself, and send it back if not.
- Capability: run checks (read plus sandboxed commands). Result: verdict. Tier: strong.
- Pays whenever the builder's output matters and nobody will read the diff. The best-supported single check in the evidence is the evaluator-optimizer loop: "one LLM call generates a response while another provides evaluation and feedback in a loop", worth it "when we have clear evaluation criteria, and when iterative refinement provides measurable value" [1]. Claude Code recommends a fresh-context reviewer so "Claude won't be biased toward code it just wrote" [5], and Cognition says its review loop works "best when the coding and review agents do not share any context beforehand" [12].
- Why it must be able to run things: MAST found explicit verifiers help, but "the presence of a verifier is not a silver bullet", citing a chess program that passed compilation but never checked game rules [13]. Without external feedback LLMs "struggle to self-correct their responses ... and at times, their performance even degrades" [21]. The quantified gains for verification all come from execution: MetaGPT's executable feedback gave +4.2% (HumanEval) and +5.4% (MBPP) Pass@1 [14]; Agentless's regression plus reproduction tests added 19 correct fixes over majority voting alone (77 to 96) [17]. I found no study that directly compares a read-only reviewer with a test-running reviewer (see "Could not verify"), so the claim "running beats reading" rests on this indirect evidence.
- Pathologies to design against: a reviewer told to find gaps "will usually report some, even when the work is sound", which leads to over-engineering [5]. LLM judges show position, verbosity and self-enhancement bias [20], and GPT-4 showed "a significant degree of self-preference bias" [22]. So: tell the judge to flag only correctness and stated-requirement gaps, give it explicit criteria, use a different model or family from the builder where possible (inference from [22] and Cognition's cross-frontier note [12]), and cap retries.
- Does not pay for trivial edits, throwaway output or when a deterministic check (build, test) already gives pass/fail. In that case run the check directly and skip the judge.
- Tier reason: a weak judge approves weak work. Judging also runs once per attempt, so the cost is bounded by the retry limit.
- Default prompt: "You decide whether the work meets the criteria you are given. Read the diff and the plan, then run the checks yourself (tests, build, git diff) and base the verdict on what you observe, not on the author's claims. Return approve or reject. On reject, list each problem with evidence, say which earlier step should redo it and what it must change. Flag only problems that affect correctness or the stated criteria; style preferences and extra features are not reasons to reject. Do not fix the problems yourself and do not reject twice for the same issue once it is addressed."

### Stage: deliver

#### reporter (SITUATIONAL)
- Purpose: write the final user-facing result from the accepted work: a report, summary or answer.
- Capability: read. Result: report. Tier: balanced.
- Pays for research and review flows where the end product is a document, and when several upstream reports must become one readable answer. Anthropic's research system adds a separate CitationAgent that "pinpoints where citations belong" after the report is written [2]. A fixed final writer also keeps the orchestrator out of the output path (inference).
- Does not pay when the builder's change is the deliverable, or when the judge's last message is already the answer.
- Tier reason: composition rather than discovery (inference).
- Default prompt: "You write the final result from the material you are given. Lead with the answer, then the evidence, then open questions. Keep every claim traceable to an input and keep its source attached. Say plainly what is uncertain or unverified. Do not add new findings, do not research, and do not soften a rejected or failed result."

## 3. Roles not to have as separate agents

| Proposed role | Source | Recommendation | Reason |
|---|---|---|---|
| Product manager, CEO, CTO, project manager | MetaGPT, ChatDev [14][15] | Do not add | These paid off in simulated companies on small greenfield tasks. In MetaGPT Table 3, going from 1 role to 4 raised executability from 1.0 to 4.0 and cut human revisions from 10 to 2.5, but cost rose from $0.915 to $1.385 (+51%) [14]. The user is the product manager; the orchestrator plans. MAST reports "disobey role specification" and "task derailment" as real failure modes in such systems [13]. |
| Separate tester | MetaGPT QA, ChatDev tester [14][15] | Fold into judge (run checks), debugger for pre-build reproduction | ChatDev's ablation text says testing is "critical for Executability" [15], which supports testing, not a separate agent. Two agents (tester then reviewer) pay two hand-offs for what one run-capable judge does. |
| Programmer plus coder-reviewer pairs | ChatDev [15] | Do not add | A reviewer inside the build step is the self-review that [21] says does not work without external feedback. Use the judge. |
| Designer, documenter, refactorer, migrator | ChatDev designer [15] | Make them builder prompts | Same capability class and result shape as builder. Only the instructions differ, which is what a custom role is for. |
| Security, spec, standards reviewer | Claude Code reviewers [3][4] | Judge variants, run in parallel | Anthropic's parallelization "sectioning" fits: independent lenses on one diff [1][4]. Define them as custom roles using the judge shape. |
| Critic / devil's advocate | AutoGen-style patterns [23] | Judge | Same function. |
| Citation / fact-check agent | Anthropic [2] | Researcher includes sources; a judge can check a report | Anthropic used a separate CitationAgent; for a small flow the researcher's quote-with-URL rule gets most of the value (inference). |
| Merger / integrator | none | Builder task | Merging is writing in a worktree. |
| Summariser / context-compressor | Cognition [11] | Do not add | Cognition says a context-compression model is "hard to get right" [11]. Pass compact structured results instead. |

## 4. Flow shapes

Notation: `a -> b` runs b after a; `[a | b]` runs in parallel; `judge ->(retry n) builder` is send-back with a limit.

1. Bug fix (single agent count: 3 to 4)
   `orchestrator -> debugger -> builder -> judge ->(retry 2) builder`
   Add `search` in parallel with `debugger` only if the code area is unknown. Skip the architect: the debugger report is the plan. The reproduction becomes the check the builder and judge run [5][17].
2. Feature (4 to 6)
   `orchestrator -> [search | researcher] -> architect -> builder -> judge ->(retry 2) architect or builder`
   Let the judge send a failed design back to the architect and a failed implementation back to the builder. Drop researcher when nothing is external, and drop the architect when the diff fits in one sentence [5].
3. Code review (3 to 5, no writer)
   `orchestrator -> [judge (correctness) | judge (spec/security lens)] -> reporter`
   Parallel lenses give independent views [1][4]. Give the reviewers a clean context with the diff and criteria only [5][12]. The reporter merges and de-duplicates. No send-back target, so no retry.
4. Research report (3 to 6, no writer)
   `orchestrator -> [researcher x2 to 4 | search] -> analyst -> reporter`
   Parallel is where multi-agent earns its cost [2]. Use `analyst` only when the researchers disagree. Optionally add a `judge` after the reporter to check claims against the cited passages.
5. Refactor (4 to 6)
   `orchestrator -> search -> architect -> builder -> judge ->(retry 2) builder`
   Make the existing tests the judge's criterion. For wide mechanical changes, builders may fan out only on disjoint file sets, each in its own worktree [5]. Never overlap files.

Rule of thumb for the number of agents (partly inference; the anchors are Anthropic's effort scaling [2], OpenAI's advice to "maximize a single agent's capabilities first" [10], and the cost multipliers [2][6]):

- Start with one agent. Add a second only for a different capability class (write versus run versus read), a clean-context check, or a parallel gather.
- Count gatherers by independent questions: 1 for a fact, 2 to 4 for a comparison, more only for broad research [2].
- Exactly one writer per file set. Exactly one judge per lens.
- A flow over about six agents needs a written reason. Most coding flows should land at 3 to 5.
- Cap send-back loops at 2 (inference). MAST lists step repetition (15.7%) and not recognising completion (12.4%) among the most common failures [13].

## 5. Custom roles

A custom role is a name, a capability class, a result shape, a model tier and a fixed prompt. This mirrors how both vendors define agents. Claude Code requires `name` and `description`, with optional `tools`, `model`, `permissionMode`, `maxTurns` and `isolation: worktree` [3]. Codex requires `name`, `description` and `developer_instructions`, with optional `model` and reasoning effort [6]. OpenAI's SDK recommends a `handoff_description` so the model knows when to pick a destination [7]. CrewAI uses role, goal and backstory [24].

What a good definition contains:
- Id and a short description that states when to use it, not how it works. Claude Code uses the description to decide when to delegate [3].
- One capability class, the smallest that does the job. A tool left out "isn't in the subagent's session at all" [4]. In Codex the subagent inherits the parent sandbox and a custom agent cannot loosen it [6].
- One result shape. Pick it by what the next agent does with it:
  - plan: the next agent will act on a list of steps.
  - report: the next agent will read findings and decide.
  - change: a diff on a branch that others will check.
  - verdict: a decision that controls flow (approve, or reject with a target to send back to).
- A tier. Cheap for mechanical high-volume reading, balanced for default work, strong for planning and judging.
- Instructions of 3 to 6 sentences: the job, the exact content to hand back, and what not to do.

How to write the instructions:
- Say what to return in a form the next agent can use without asking questions. A subagent gets only its prompt and the task text, "not the parent's conversation history" [4]. So ask for paths, quotes, commands and decisions, not narrative.
- Put the goal in the task, never in the role prompt. A role is reused across runs.
- State boundaries explicitly. Anthropic found vague tasks produced duplicated work, and recommends each delegation carry "objective, output format, tool guidance, boundaries" [2].
- Give a stopping rule. Without one, agents search endlessly or repeat steps [2][13].
- Prefer a narrow specialist to a broad one. OpenAI: "Prefer specialized agents over one general-purpose agent" [8].
- Ask for evidence, not assertions. "Have Claude show evidence rather than asserting success" [5].

Common mistakes:
- A role that is just a new name for an existing shape (a "documenter" is a builder). Use a prompt variant instead of a new capability class.
- Giving a reviewer write access, or a writer sign-off on its own work [21][22].
- A judge prompt that rewards finding problems with no threshold, which leads to over-engineering [5].
- Descriptions that are long. Claude Code warns when combined agent descriptions exceed 15,000 tokens and wants detail in the body [3].
- A prompt that restates the repo's conventions or the user's goal. It goes stale and wastes tokens.
- No send-back target on a verdict role, or no retry limit.

### Checklist for generating a custom role

1. Give a lowercase single-word id that is not already taken, and a display name.
2. Write one sentence of purpose that names a job no existing role does. If an existing role has the same capability class and result shape, change its prompt instead of adding a role.
3. Choose the capability class: read (default), run checks (only if it must observe by executing), write (only if it must change files, and only one writer per file set).
4. Choose the result shape: plan, report, change or verdict, by what the next agent will do with it.
5. Choose the tier: cheap for lookup, balanced for default, strong for planning or judging. A judge should not be cheaper than the builder.
6. Write 3 to 6 sentences: the job, what to hand back (list the fields), what not to do, and when to stop.
7. Do not include the user's goal, file paths of this project, or conventions that belong in the task.
8. For a verdict role, say what counts as a reject, what to name as the earlier step to redo, and not to reject for style.
9. State how it differs from the nearest built-in role, in one line.
10. Check it with a worked example: could the next agent act on this output alone?

## 6. Evidence by source

[1] Anthropic, "Building effective agents", https://www.anthropic.com/engineering/building-effective-agents. Read in full.
- Defines five workflows: "Prompt chaining decomposes a task into a sequence of steps, where each LLM call processes the output of the previous one"; "Routing classifies an input and directs it to a specialized followup task"; parallelization as sectioning and voting; orchestrator-workers; evaluator-optimizer ("one LLM call generates a response while another provides evaluation and feedback in a loop").
- Relied on: "we recommend finding the simplest solution possible, and only increasing complexity when needed"; agents bring "higher costs, and the potential for compounding errors"; coding is promising because "Code solutions are verifiable through automated tests".

[2] Anthropic, "How we built our multi-agent research system", https://www.anthropic.com/engineering/multi-agent-research-system. Read in full via a fetch tool summary (the answers come from the tool's extraction, not from my own page read).
- Relied on: orchestrator-worker with LeadResearcher, parallel subagents and a CitationAgent; about 4x tokens for agents and about 15x for multi-agent versus chat; token use alone explains 80% of BrowseComp variance; Opus 4 lead plus Sonnet 4 subagents beat single-agent Opus 4 by 90.2% on the internal research eval; effort-scaling rules (1 agent / 3-10 calls; 2-4 subagents / 10-15 calls; more than 10 for complex research); early failures (50 subagents for simple queries, endless web search, duplicated work from vague tasks, SEO farms); poor fits (shared-context domains and coding, with "fewer truly parallelizable tasks"); delegation needs objective, output format, tool guidance, boundaries; synchronous execution means the lead "can't steer subagents".
- LLM-as-judge: a single call scoring 0.0 to 1.0 plus pass/fail was most consistent and matched humans best; human testing remained essential.

[3] Claude Code docs, "Create custom subagents", https://code.claude.com/docs/en/sub-agents.
- Built-ins: `Explore` (read-only; file discovery, code search, codebase exploration), `Plan` (read-only; codebase research during plan mode), `general-purpose` (every tool; "Complex research, multi-step operations, code modifications"). Explore and Plan are one-shot.
- Use the main conversation for back-and-forth, shared-context phases ("plan, implement, test") and quick changes; use subagents for verbose output, tool restrictions and self-contained work that can "return a summary".
- Frontmatter fields, 15,000-token description warning, default 20 concurrent subagents, nesting up to three layers by default. The page was truncated at its end (last ~11,000 characters not read).

[4] Claude Agent SDK, "Subagents in the SDK", https://code.claude.com/docs/en/agent-sdk/subagents (redirected from platform.claude.com).
- Four benefits: context isolation, parallelization, specialized instructions, tool restrictions. Example `code-reviewer` (Read, Grep, Glob) and `test-runner` (Bash, Read, Grep). Common tool sets: read-only analysis, test execution, code modification. The only content passed parent to child is the prompt string. Spend, depth and concurrency caps (`maxBudgetUsd`, depth 3, 20 concurrent).

[5] Claude Code docs, "Best practices", https://code.claude.com/docs/en/best-practices.
- "Give Claude a way to verify its work"; "Explore first, then plan, then code" with the "one sentence" skip rule; Writer/Reviewer pattern with fresh context; "Add an adversarial review step" with the caveat that a reviewer "will usually report some, even when the work is sound"; subagents for investigation; `/batch` splits a change over 5 to 30 subagents each in its own worktree.

[6] Codex, "Subagents", https://learn.chatgpt.com/docs/agent-configuration/subagents (redirected from developers.openai.com/codex/subagents).
- Built-in roles: `default` ("general-purpose fallback agent"), `worker` ("execution-focused agent for implementation and fixes"), `explorer` ("read-heavy codebase exploration agent"). "subagent workflows consume more tokens than comparable single-agent runs." Parallel read-heavy work first; more care with parallel write-heavy work. "Subagents inherit your current sandbox policy." Custom agents: `name`, `description`, `developer_instructions`. Concurrency cap `agents.max_concurrent_threads_per_session` (alias `agents.max_threads`). `max_depth` is not documented on this page.

[7] OpenAI Agents SDK, "Handoffs", https://openai.github.io/openai-agents-python/handoffs/. "Handoffs allow an agent to delegate tasks to another agent"; represented as tools; the receiving agent sees prior history unless an `input_filter` changes it; `handoff_description` helps the model choose.

[8] OpenAI Agents SDK, "Orchestrating multiple agents", https://openai.github.io/openai-agents-python/multi_agent/. Agents as tools (manager keeps control) versus handoffs (specialist takes over); code-orchestrated patterns: structured outputs for routing, chaining, evaluator loop, parallel execution; "Prefer specialized agents over one general-purpose agent"; code orchestration is "more deterministic and predictable, in terms of speed, cost and performance".

[9] OpenAI Agents SDK, "Guardrails", https://openai.github.io/openai-agents-python/guardrails/. Input guardrails run only for the first agent and output guardrails only for the final agent; parallel mode gives best latency but the agent may already have used tokens, blocking mode "guarantees that the expensive model does not start"; example of a fast cheap model guarding an expensive one.

[10] OpenAI, "A practical guide to building agents", https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf. Text extracted locally with pdftotext. "Our general recommendation is to maximize a single agent's capabilities first"; split when prompts have many conditional branches or tools overlap ("Some implementations successfully manage more than 15 well-defined, distinct tools while others struggle with fewer than 10 overlapping tools"); two patterns, Manager (agents as tools) and Decentralized (handoffs).

[11] Cognition, "Don't Build Multi-Agents", https://cognition.com/blog/dont-build-multi-agents (dated 06.12.25 on the page). Principles: "Share context, and share full agent traces, not just individual messages" and "Actions carry implicit decisions, and conflicting decisions carry bad results"; Flappy Bird example; recommends "a single-threaded linear agent" with a history-compression model for long tasks, "hard to get right".

[12] Cognition, "Multi-Agents: What's Actually Working", https://cognition.com/blog/multi-agents-working (dated 04.22.26 per the search result; the page fetch did not show the date). This is the rebuttal/update. What works: read-only subagents; a clean-context reviewer ("best when the coding and review agents do not share any context beforehand"); a strong "smart friend"; a manager that spawns children. Limits: "most of the sexy ideas ... still don't see meaningful adoption"; unstructured swarms "mostly a distraction"; managers "overly prescriptive"; review loop slow. Quoted number: Devin Review catches "an average of 2 bugs per PR", roughly 58% severe (self-reported by the vendor).

[13] Cemri et al., "Why Do Multi-Agent LLM Systems Fail?" (MAST), https://arxiv.org/abs/2503.13657 (v3, NeurIPS 2025 Datasets and Benchmarks). Abstract: 1600+ annotated traces, 7 frameworks, 14 failure modes in 3 categories (system design, inter-agent misalignment, task verification), annotator kappa 0.88. From the paper body (text extracted locally): category shares 44.2% (FC1 system design) and 32.3% (FC2 inter-agent), with FC3 modes 6.20% + 8.20% + 9.10%; step repetition 15.7%; not recognising completion 12.4%; reasoning-action mismatch 13.2%; "Insight 3. Multi-Level Verification is Needed"; "the presence of a verifier is not a silver bullet"; a role/prompt-spec intervention gave +9.4% task success for ChatDev with the same model.

[14] Hong et al., "MetaGPT", https://arxiv.org/abs/2308.00352 (ICLR 2024). Five roles (Product Manager, Architect, Project Manager, Engineer, QA Engineer). Table 3 role ablation (SoftwareDev, human-rated executability 1 to 4, GPT-4): Engineer only: 1 agent, $0.915, 10 revisions, executability 1.0; + Product: $1.059, 6.5, 2.0; + Product + Architect: $1.204, 4.0, 2.5; + Product + Project: $1.251, 3.5, 2.0; all four: $1.385, 2.5, 4.0. Executable feedback: +4.2% HumanEval and +5.4% MBPP Pass@1; human revision cost 2.25 to 0.83.

[15] Qian et al., "ChatDev", https://arxiv.org/abs/2307.07924. Roles CEO, CTO, programmer, reviewer, tester (Figure 2 phases: design, coding, code complete, code review, testing). Ablation text: "the code complete phase enhances Completeness, with testing critical for Executability". The ablation table values were garbled in the text extraction and are not used.

[16] Fourney et al., "Magentic-One", https://arxiv.org/abs/2411.04468. Orchestrator plus WebSurfer, FileSurfer, Coder, ComputerTerminal. Ablation (GAIA validation, GPT-4o): "without the full ledgers, performance drops by 31%"; removing any single worker "reduces performance by between 21% (Coder, Executor) to 39% (FileSurfer)". Agents sometimes compensated for a missing agent in other ways.

[17] Xia et al., "Agentless", https://arxiv.org/abs/2407.01489. Three phases (localization, repair, patch validation) with no agent loop; SWE-bench Lite 32.00% (96 fixes) at $0.70. Table 4 patch-selection ablation: majority voting 77 (25.67%, $0.00); + regression tests 81 (27.00%, $0.01); + reproduction tests 96 (32.00%, $0.25).

[18] Yang et al., "SWE-agent", https://arxiv.org/abs/2405.15793. Single agent with an agent-computer interface; 12.5% pass@1 on SWE-bench; the claim relied on is only that interface design affects agent performance (abstract).

[19] Wang et al., "OpenHands", https://arxiv.org/abs/2407.16741. Single agent built on the CodeAct architecture, acting through shell, Python and browser actions. No role ablation relied on.

[20] Zheng et al., "Judging LLM-as-a-Judge with MT-Bench and Chatbot Arena", https://arxiv.org/abs/2306.05685. Names position, verbosity and self-enhancement biases and limited reasoning ability; GPT-4 judge agreement with humans "exceeding 80%", the same as human-human agreement.

[21] Huang et al., "Large Language Models Cannot Self-Correct Reasoning Yet", https://arxiv.org/abs/2310.01798. "LLMs struggle to self-correct their responses without external feedback, and at times, their performance even degrades after self-correction." Scope: reasoning tasks, intrinsic self-correction.

[22] Wataoka et al., "Self-Preference Bias in LLM-as-a-Judge", https://arxiv.org/abs/2410.21819. "GPT-4 exhibits a significant degree of self-preference bias"; hypothesised cause is that models favour outputs with lower perplexity, i.e. more familiar text.

[23] Wu et al., "AutoGen", https://arxiv.org/abs/2308.08155. Agents are "customizable, conversable" and can combine LLMs, humans and tools. No role ablation relied on.

[24] CrewAI docs, "Agents", https://docs.crewai.com/en/concepts/agents. An agent has `role`, `goal`, `backstory` (all required); `allow_delegation` defaults to False; examples are narrow specialists.

## 7. Could not verify

- A head-to-head study of a read-only reviewer versus a test-running reviewer. The "running beats reading" claim is built from indirect evidence [14][17][13][21]; Agentless's gain is from test-based patch selection, not from a reviewer agent.
- Role-removal numbers for ChatDev (Table 4 values were scrambled in the extraction) and for SWE-agent, OpenHands, AutoGen and CrewAI (no role ablation found in the parts I read).
- Magentic-One's "simple Orchestrator" ablation number: I only saw the figure caption, not the value; the 31% and 21% to 39% figures are from the text.
- Quantified self-preference effect sizes in [22]; only the abstract's qualitative finding is used.
- LangGraph supervisor and swarm documentation: both pages returned only a redirect, so nothing from LangGraph is cited.
- Cross-model judging as a fix for self-preference: [22] shows the bias but I did not find a study that tests a different-model judge as a remedy; Cognition [12] describes cross-frontier use as a capability router. The advice is inference.
- Mapping of Codex model names (gpt-6.1-sol, gpt-6-astra, gpt-6-luna) to strong, balanced or cheap tiers. Not in any source I read; assign tiers from the account's own model list.
- Whether the "run checks" sandbox in ttm includes web access. The brief lists web only under the read class.
- The Cognition 2026 post's publication date and "58% severe" figure come from a fetch summary and a search result; treat as vendor self-report.
- Claude Code behaviour of the built-in `Explore` model: the docs say it follows the main conversation's model (with a documented Fable-to-Opus case), and show an override to `haiku`. The "cheap by default" claim for search is a recommendation, not a documented default.
