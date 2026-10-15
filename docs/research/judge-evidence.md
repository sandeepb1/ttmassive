# Judge role: evidence for execution and for a different model

Researched 2026-10-15. Most pages were read through WebFetch, which returned abstracts for some papers and full-text excerpts for others. Each number below is marked with where it came from. Where I only saw an abstract or a search summary I say so.

## Verdicts

1. **Claim 1, a reviewer that runs code or tests beats one that only reads: supported indirectly, and mixed.** Execution feedback clearly beats no feedback in correction loops (Self-Debugging, CRITIC), but the one direct patch-level comparison (R2E-Gym) found tests-only and read-only verifiers plateau at the same level (43.7% vs 42.8%) and that combining them is better (51.0%). A newer controlled study of repair agents found that forbidding execution cost only 1.25 points.
2. **Claim 2, a different model reduces self-preference bias: supported indirectly.** Self-preference is measured and real for some judges, and cross-family panels reduce spread. No paper I found directly measures same-model vs different-model review of code for catching bugs. Much of the raw self-preference gap is genuine quality difference, and some bias remains after controlling for quality.

## Claim 1: executing beats reading

### Direct comparisons (same task, with and without execution)

**Self-Debugging (Chen et al., arXiv 2304.05128), Tables 2 and 3.** Compared feedback with unit-test execution against feedback with only the model's own explanation, no execution. Accuracy %, Codex / GPT-3.5 / GPT-4:
- MBPP: baseline 61.4 / 67.6 / 72.8. With unit-test execution feedback (Table 2c "UT"): 69.4 / 72.2 / 80.6. With explanation only, no execution (Table 3): 64.4 / 68.2 / 76.0.
- TransCoder: baseline 80.4 / 89.1 / 77.3. With unit-test execution (Table 2b "UT"): 91.6 / 92.7 / 88.8. Explanation only (Table 3): 83.9 / 89.1 / 78.0.
- Reading does not help much; running does. On GPT-4 TransCoder, execution gives +11.5 points over baseline versus +0.7 for explanation only.
- Caveat: this is a code writer repairing its own program, not a reviewer verdict. Tasks are MBPP/TransCoder snippets, not repository patches. The paper itself notes the MBPP explanation comparison is partly confounded by 3 vs 6 prompt exemplars (Appendix C.2).

**CRITIC (Gou et al., arXiv 2305.11738), Tables 1-3.** Compared verification with the external tool against the same loop with the model critiquing alone ("w/o Tool"). Program synthesis, exact match, ChatGPT: GSM8k 78.2 vs 77.0, SVAMP 83.3 vs 82.0, TabMWP 89.0 vs 87.0. Text-davinci-003 GSM8k 72.2 vs 68.3, where the no-tool loop drops below the base program (-1.8 vs +2.1). The authors say that without interpreter feedback, correction is "limited and unstable." Caveat: gaps are small on code-like math tasks (1 to 4 points) and larger on QA with a search tool.

**Huang et al., "Large Language Models Cannot Self-Correct Reasoning Yet" (arXiv 2310.01798).** Intrinsic self-correction with no external signal lowered accuracy, for example GPT-4 on GSM8K 95.5 to 89.0 and GPT-3.5 on CommonSenseQA 75.8 to 41.8. This paper ran no code experiment. It cites Chen et al. for the point that where an executor can check behaviour it acts as a "perfect verifier." Indirect for code, direct for "a model reading its own work and judging it without ground truth makes things worse."

**R2E-Gym (Jain et al., arXiv 2504.07164), sections 4.2-4.4, Table 4.** The only patch-level comparison I verified. Verifying 26 SWE-agent candidates per SWE-Bench Verified instance (Pass@1 34.4%):
- Execution-based verifier (generated reproduction tests plus regression tests): plateaus at 43.7%.
- Execution-free verifier (a trained model reads the issue, trajectory and patch, no running): plateaus at 42.8%.
- Hybrid: 51.0% at Best@26 (49.4% at Best@16).
- Why neither alone is enough (section 4.2-4.3): the tests often fail to separate good from bad patches (under 20% of tests discriminate for most problems), and "toxic" tests that pass wrong patches and fail right ones reach up to 10% for some problems. The read-only verifier discriminates better but leans on agent "thoughts" as a shortcut.
- Caveat: the read-only verifier here is a trained model, not a general model with the repo open. The tests are model-written, not the project's own suite.

**"To Run or Not to Run" (arXiv 2606.26978, ISSTA 2026; abstract only was read).** On 200 SWE-bench instances with Claude Code, Codex and OpenCode, unrestricted vs prohibited execution differed by 1.25 points of resolve rate, not significant, with large token and time savings from prohibiting. Gains from execution were "concentrated rather than uniform." This is about an agent writing a patch, not reviewing one, but it is the closest controlled "run vs do not run" result for current frontier agents, and it cuts against "execution is always better."

### Indirect evidence

- **Execution-free verifiers also work (secondary summary plus abstracts).** SWE-RM (arXiv 2512.21919, abstract only): execution-free reward model raised Qwen3-Coder-Flash from 51.6% to 62.0% on SWE-Bench Verified with test-time scaling. Meta's Agentic Code Reasoning (arXiv 2603.01896, seen through a search summary, not the paper text): structured reading without execution reached 93% patch-equivalence accuracy with Opus-4.5 vs 86% for an unstructured single shot. These show reading can be strong when it is structured, not that it beats running.
- **CodeJudge (arXiv 2410.02184, abstract only).** Evaluates code correctness "without test cases" with step-by-step reasoning and beats other LLM evaluators. It does not compare against execution, so it says nothing either way on Claim 1.
- **CRBugBench (arXiv 2610.13668, partial read).** Best review agent catches 71.9% of known bugs in its top 5 comments (opus-4.6 was 47.7%). Agents run code in only about 8% of turns (9.7% for claude-opus-5, 6.9% for gpt-5.6-sol). The paper reports no execution ablation, so it shows that current reviewers mostly read, and that they do well, but cannot tell us whether running would add more.
- **ACECODER (arXiv 2502.01718, abstract only).** Reward models trained on test-case pass rates improve best-of-32; no explicit run-vs-read comparison.

### Bottom line for Claim 1

Direct evidence that execution helps: writer-side repair loops (Self-Debugging, CRITIC), where the gain from running is clear and the gain from reading alone is small or negative. Direct evidence at patch level: execution and reading verifiers are about equal alone and complementary together (R2E-Gym). Direct evidence against "always run": the 2026 execution-ablation study. I found no study that gives an LLM reviewer of a diff the same repo with and without a shell and measures bug-catch rate. That specific comparison is absent.

## Claim 2: a different model reduces self-preference

### Size of the bias

- **Panickssery et al. (arXiv 2404.13076), Sec. 2.3-2.4, Table 7.** Summarization, pairwise, no fine-tuning. GPT-4 self-recognition 0.672 (XSUM) and 0.747 (CNN); self-preference 0.705 and 0.912. GPT-3.5 0.535 / 0.481 recognition, 0.582 / 0.431 preference. Llama-2-7b near chance (about 0.51). GPT-4 reaches 73.5% telling itself from two LLMs and humans. After fine-tuning on 500 examples, GPT-3.5 and Llama 2 exceed 90% self-recognition, and self-preference rises linearly with self-recognition (Kendall tau 0.74 and 0.82 for fine-tuned GPT-3.5 in Table 1). Caveats from Sec. 5.2: no ground-truth quality labels were collected, so justified preference cannot be separated from bias; the authors say the causal story is supported but not fully validated.
- **Zheng et al., MT-Bench (arXiv 2306.05685), Sec. 3.3.** GPT-4 favours itself with a 10% higher win rate, Claude-v1 25% higher, GPT-3.5 does not. The authors state that with limited data and small differences they cannot tell whether self-enhancement bias exists. Same paper: GPT-4 agrees with humans 85% (non-tie votes), versus 81% human-human.
- **Wataoka et al. (arXiv 2410.21819).** GPT-4 bias score 0.520 on their equal-opportunity metric (demographic-parity variant 0.749); GPT-3.5-Turbo 0.191 on the parity variant; other models near zero; some small models negative. They attribute it to perplexity: judges rate low-perplexity (familiar) text higher than humans do. Claude and Gemini were not evaluated, and perplexity could not be measured for GPT-4 itself.
- **Spiliopoulou et al., "Play Favorites" (arXiv 2508.06709, abstract only).** With a third-party quality control and 9 judges: GPT-4o and Claude 3.5 Sonnet score their own outputs higher, and also rate other models in their own family higher. No effect sizes seen. This is the source for "family" bias, i.e. switching to a sibling model may not help.

### Does a different model or family help

- **Evidence that a cross-family panel reduces judge-specific bias:** PoLL (Verga et al., arXiv 2404.18796, Tables 1-2, Sec. 4.4). Panel of Command R, Haiku and GPT-3.5 vs a single GPT-4 judge: Cohen kappa vs humans 0.763 vs 0.627 (NQ), 0.906 vs 0.841 (TQA), 0.867 vs 0.830 (HotpotQA); Arena-Hard Pearson 0.917 vs 0.817. Lowest spread across judges (std dev 2.2 against up to 6.1 for a single judge). About 7 to 8 times cheaper. In their Bamboogle figure the largest positive score delta for each model occurs when it judges itself, and on Arena-Hard GPT-4 ranks another GPT-4 variant 2nd when its true position is 4th. Caveat: tasks are QA and chat, not code, and the paper reports no per-judge self-preference statistic. Panel gains are mixed with the effect of using several judges at all.
- **Evidence for a quality caveat:** Chen et al., "Do LLM Evaluators Prefer Themselves for a Reason?" (arXiv 2504.03846, abstract only). On verifiable tasks including code generation across 7 model families, stronger models' self-preference is mostly legitimate (they really are better), but harmful self-preference persists when the model was wrong as a generator, and is more pronounced for stronger models. Its named mitigation is inference-time scaling (long chain of thought before judging), not switching models.
- **Evidence that bias is partly an artifact:** "Are LLM Evaluators Really Narcissists?" (arXiv 2601.22548, ICML 2026, abstract only). With a baseline for evaluator quality, only 51% of previously flagged examples keep statistical significance, though those carry 89.6% of the effect. So self-preference is real but overstated.
- **Code-specific:** "Bias in the Loop" (arXiv 2604.16790, partial read) probed judges for software tasks with a "your own model family wrote A" label. Effects on code generation, repair and test generation were small and mixed (for example Qwen3-4B CodeGen 67.50 to 67.44; Qwen2.5-Coder-3B CodeGen 60.99 to 63.54). The probe is a text label, not real same-model output, so it does not measure true stylistic familiarity. This is the only coding-specific self-enhancement data I saw, and it shows no consistent effect.
- **Review benchmark (CRBugBench, arXiv 2610.13668).** Used a three-family judging panel specifically "to mitigate judge biases towards same-family models" and tested ensembles of different models: a different-model Ensemble@3 gave recall@5 66.8 (+1.2) vs 70.4 (+4.7) for same-model Ensemble@5 on one track. This is about ensembling reviewers, not about reducing bias, and favours same-model in that single cell.

### Bottom line for Claim 2

The bias exists and is largest for GPT-4-class judges on summarization and chat. A cross-family panel measurably reduces judge-specific spread on QA and chat. No study I verified compares a coding agent reviewed by its own model against one reviewed by another model on bug-catch rate. Same-family bias (GPT-4o, Claude 3.5 Sonnet) means a sibling model is a weaker substitute than a different vendor. A stronger judge can still be harmfully biased on its own mistakes.

## Codex model tiers (from `codex app-server` model/list, run 2026-10-15)

Descriptions are verbatim from the installed Codex. They contain no prices or benchmark figures, so tiers are an interpretation of the wording.

| Model | Own description | Suggested tier | Evidence for the tier |
|---|---|---|---|
| gpt-6-astra | "Frontier intelligence for the most demanding work." | strong | Only model described as frontier / most demanding. |
| gpt-6.1-sol (default) | "Latest workhorse model for coding and everyday work." | strong-to-balanced (default for coding work) | "Workhorse" for coding, marked default; ranked below Astra by the "frontier" wording. |
| gpt-6-sol | "Previous generation workhorse model." | balanced | Same "workhorse" label, one generation older. |
| gpt-6-luna | "Fast and affordable model for easier tasks." | cheap | "Fast and affordable", "easier tasks". Efforts stop at max (no ultra). |
| gpt-5.6-sol | "Older generation workhorse model." | balanced (older) | "Workhorse", two generations older. |
| gpt-5.6-terra | "Older balanced model for straightforward work." | balanced-to-cheap (older) | Literally "balanced" but scoped to "straightforward work". The Terra-vs-Sol ordering is inferred from the wording only. |
| gpt-5.6-luna | "Older fast and efficient model." | cheap (older) | "Fast and efficient." No ultra effort. |

All models list effort levels up to max; astra, sol and terra models also list ultra, luna models do not. Nothing in the output ranks capability numerically.

## What this means for the design

1. **Give the judge a way to run read-only checks, but make it optional per flow and do not treat it as always better. (Evidence-backed, partly.)** R2E-Gym shows run-based and read-based verification plateau equally and the combination gains 7 to 8 points. Self-Debugging and CRITIC show running beats reading for correction. The 2026 execution ablation shows frontier agents lose little without execution. So offer test, build and `git diff` as tools, and default to on where the project has a test suite.
2. **Expect the tests to be unreliable evidence, and let the judge weigh them with its reading. (Evidence-backed.)** R2E-Gym found tests that pass wrong patches and fail right ones (up to 10% for some problems), and under 20% of tests discriminating. Present test output to the judge as one input, not a verdict, and prefer the project's own existing tests over tests the writer just added.
3. **Do not let the judge rely on the writer's explanation or reasoning. (Evidence-backed.)** The R2E-Gym read-only verifier over-weights agent thoughts; Huang et al. show a model judging its own reasoning without ground truth degrades. Give the judge the diff, the task and the check output, not the writer's self-assessment.
4. **Default the judge to a different vendor family than the writer, with sibling models as second choice. (Mostly inference.)** Self-preference is measured (Panickssery, Zheng, Wataoka), and "Play Favorites" shows family bias for GPT-4o and Claude 3.5 Sonnet. No code-review study confirms the gain, and Chen et al. shows that bias on the model's own wrong answers survives better models, so treat this as a cheap precaution, not a guarantee.
5. **Do not pick a weaker judge just because it is a different family. (Inference.)** Chen et al. and the narcissist paper show that quality differences explain much of apparent self-preference; a cheap-tier judge can miss bugs the writer made. Keep the judge at the same tier as the writer or one tier below, and use the strong tier for high-stakes flows. The Codex tier table above supports the assignment of tiers.
6. **Allow an optional panel of two or three judges from different families for high-risk changes. (Partly evidence-backed.)** PoLL supports panels across families for lower spread and cost on QA and chat; there is no coding-specific replication. Use it only where cost allows, and record disagreement rather than only a majority vote.
7. **Give the judge room to think before deciding. (Evidence-backed.)** Chen et al. report that long chain of thought before judging reduces harmful self-preference. Use a higher effort setting for the judge than for routine steps.

## Sources

| URL | Claim relied on |
|---|---|
| https://arxiv.org/abs/2404.13076 (HTML read) | Self-recognition and self-preference numbers (Table 7, Table 1), linear link, no ground-truth quality labels (Sec. 5.2). |
| https://arxiv.org/abs/2306.05685 | GPT-4 self-enhancement +10% win rate, Claude-v1 +25%, authors unsure it exists; GPT-4 85% agreement with humans (Table 5). |
| https://arxiv.org/abs/2410.21819 | GPT-4 bias 0.520; perplexity explanation; Claude and Gemini not evaluated. |
| https://arxiv.org/abs/2404.18796 | PoLL panel vs GPT-4 kappa, Pearson, spread, cost (Tables 1, 2, Sec. 4.4-4.5). |
| https://arxiv.org/abs/2410.02736 | Abstract only: 12 biases, CALM framework. No self-enhancement numbers seen. |
| https://arxiv.org/abs/2304.05128 | Self-Debugging Tables 2 and 3 (execution vs explanation-only). |
| https://arxiv.org/abs/2305.11738 | CRITIC with vs without tool (Tables 1-3). |
| https://arxiv.org/abs/2310.01798 | Intrinsic self-correction degrades; defers to execution feedback for code. |
| https://arxiv.org/abs/2504.07164 | R2E-Gym execution-based 43.7%, execution-free 42.8%, hybrid 51.0%; toxic tests. |
| https://arxiv.org/abs/2512.21919 | SWE-RM abstract: 51.6% to 62.0% with execution-free reward model. |
| https://arxiv.org/abs/2410.02184 | CodeJudge abstract only: judges code without test cases. |
| https://arxiv.org/abs/2606.26978 | Abstract only: 1.25-point resolve-rate gap, unrestricted vs prohibited execution. |
| https://arxiv.org/html/2610.13668 | CRBugBench: 71.9% top-5, 8% of turns execute, no execution ablation, three-family judging panel. |
| https://arxiv.org/abs/2603.01896 | Seen through search summary only: 93% vs 86% patch equivalence, execution-free. |
| https://arxiv.org/abs/2502.01718 | Abstract only: best-of-32 with reward model trained on test pass rates. |
| https://arxiv.org/abs/2504.03846 | Abstract only: legitimate vs harmful self-preference on verifiable tasks including code; inference-time scaling mitigates. |
| https://arxiv.org/abs/2508.06709 | Abstract only: GPT-4o and Claude 3.5 Sonnet self and family bias. |
| https://arxiv.org/abs/2601.22548 | Abstract only: 51% of flagged examples significant, 89.6% of mass. |
| https://arxiv.org/html/2604.16790v1 | Self-enhance probe on code tasks: small, mixed effects. |
| `codex app-server` `model/list` output | Model descriptions in the tier table. |

## Could not verify

- No direct study of an LLM reviewer of a code diff, with and without ability to run the code, measured on bug-catch rate. The nearest are R2E-Gym (verifier) and the 2606.26978 repair ablation.
- No study of coding agents reviewed by the same model vs a different model. The code-specific self-enhancement probe uses a text label, not real same-model output.
- Numbers for SWE-Gym, Agentless, CodeRM, CodeT, LEVER, Reflexion and "Can LLMs replace manual review" were not fetched, so I make no claim from them.
- Abstract-only papers (SWE-RM, CodeJudge, 2606.26978, ACECODER, 2504.03846, 2508.06709, 2601.22548, Justice or Prejudice): I did not see the body tables. The Agentic Code Reasoning figures come from a search summary, not the paper.
- The DeepSWE and Agentic Rubrics numbers in a search summary were not opened.
- Claude model tiers (opus, sonnet, haiku) were not researched here; Codex tiers use the wording of the descriptions only, with no price or benchmark data.
- "Lower intra-model bias" for PoLL is the authors' claim from the abstract; the body gives spread and one example, not a per-judge self-preference statistic.
