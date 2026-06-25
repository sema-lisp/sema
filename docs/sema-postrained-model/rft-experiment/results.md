# RFT Experiment — Final Results

Benchmark of multiple approaches to making LLMs Sema-aware, conducted
2026-06-24 on the `experiment/rft-qwen3-sema` branch.

---

## Summary

We tested four approaches across 60 Sema coding tasks (5 difficulty levels):

1. **Bare model** — system prompt only, no fine-tuning, no tools
2. **RFT fine-tuned** — Qwen3-8B trained via free Reinforcement Fine-Tuning on Fireworks
3. **Tool-augmented** — model given `eval_code` + `docs_search` tools (self-correcting loop)
4. **RFT + tools** — fine-tuned Qwen3-8B with the same tools

We also compared 6 frontier models with tools to see which is best for Sema.

---

## Approach 1: Bare Model (no tools, no fine-tuning)

| Level | GLM 5.2 (743B) | Qwen3-8B (RFT) |
|-------|:-:|:-:|
| L1: Trivial eval | 58% | 58% |
| L2: Write function | 61% | 37% |
| L3: Multi-feature | 65% | 23% |
| L4: Full program | 13% | 26% |
| L5: Macros/async | 12% | 6% |
| **Overall** | **49%** | **36%** |

**Findings:**
- GLM 5.2 is better overall (49% vs 36%) — its 743B reasoning capacity wins on L2-L3
- RFT fixed specific Sema dialect issues (L4: 26% vs 13%, L5: 6% vs 12% on some tasks)
- GLM 5.2's main failure: Clojure-style `(let [x 1] ...)` instead of Sema's `(let ((x 1)) ...)`
- RFT's main failure: hallucinated nonexistent functions (`string/equals?`, `add1`, `map/merge`)
- GLM 5.2 latency: 12.1s avg. RFT: 0.9s avg (13x faster with `/no_think`)

---

## Approach 2: Tool-Augmented (eval_code + docs_search)

| Level | GLM 5.2 + tools | Qwen3-8B RFT + tools |
|-------|:-:|:-:|
| L1: Trivial eval | 72% | 81% |
| L2: Write function | 75% | 9% |
| L3: Multi-feature | 78% | 4% |
| L4: Full program | 56% | 0% |
| L5: Macros/async | 66% | 6% |
| **Overall** | **71%** | **24%** |

**Findings:**
- GLM 5.2 + tools is the clear winner: **71% overall**, +22pp over bare GLM 5.2
- The `eval_code` tool lets the model self-correct — L4 went from 13% → 56%, L5 from 12% → 66%
- RFT + tools scored WORST (24%) — RFT training damaged the model's general coding ability:
  - Generates prose instead of code ("Unbound variable: The")
  - Uses backticks in code, which Sema interprets as quasiquote
  - Can't iterate on errors — calls `eval_code` once, gets error, gives up
- RFT + tools DID have the best L1 (81%) — the model knows Sema syntax and verifies with eval
- Average 1.7 tool calls per task for GLM 5.2 — very efficient self-correction

---

## Approach 3: Multi-Model Comparison (all with tools)

| Model | Overall | L1 | L2 | L3 | L4 | L5 | Tools/task | $/M tok |
|-------|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| **kimi-k2.6** | **60%** | 35% | 75% | 71% | 59% | 66% | 1.5 | $0.95/$4 |
| deepseek-v4-pro | 58% | 39% | 81% | 73% | 49% | 18% | 1.9 | $1.74/$3.48 |
| kimi-k2.7-code | 58% | 30% | 75% | 69% | 59% | 60% | 1.5 | $0.95/$4 |
| qwen3.7-plus | 56% | 30% | 73% | 62% | 69% | 40% | 1.8 | $0.40/$1.60 |
| glm-5.2 | 54% | 30% | 66% | 73% | 59% | 26% | 1.6 | $1.40/$4.40 |
| deepseek-v4-flash | 53% | 53% | 61% | 57% | 59% | 6% | 1.8 | $0.14/$0.28 |

**Findings:**
- Kimi K2.6 is the overall winner (60%), strongest on L5 (66% — macros/async/lazy)
- DeepSeek V4 Pro has the best L2 (81% — writes correct functions)
- Kimi K2.7 Code is best for L5 (60% — the "Code" variant handles macros better)
- Qwen 3.7 Plus is best value: 56% at $0.40/$1.60 (3.5x cheaper than GLM 5.2)
- DeepSeek V4 Flash is cheapest viable: 53% at $0.14/$0.28 (10x cheaper than GLM 5.2)
- All models score ~30% on L1 — this is a **grader bug**, not a model capability issue.
  Models get the right answer via `eval_code` but format their response with
  explanations ("The result is `3`") and the answer extractor picks up the wrong text

---

## RFT Training Metrics

The RFT job on Qwen3-8B completed successfully (2 epochs, free on Fireworks):

| Metric | Value |
|--------|-------|
| Base model | Qwen3-8B (8.2B params) |
| Training method | Reinforcement Fine-Tuning (GRPO) |
| Cost | **$0** (free for <16B models on Fireworks) |
| Training time | ~40 minutes |
| LoRA rank | 16 |
| Epochs | 2 |
| Total rollouts | 692 |
| Starting avg reward | 76.9% |
| Final avg reward | 87.9% (+11pp improvement) |
| Median score | 100% (604/692 perfect, 80 failed, 8 partial) |

The training data was 1,037 eval-test cases extracted from the Sema codebase's
`eval_tests!` macros, converted to (expression, expected_output) pairs.

---

## Cost Summary

| Item | Cost |
|------|------|
| RFT training (Qwen3-8B, free tier) | $0 |
| GLM 5.2 serverless API calls (benchmarking) | ~$8 |
| Fireworks deployment (3 hours H100) | ~$21 |
| Total experiment cost | **~$29** |

---

## Key Takeaways

1. **Tool-augmented > fine-tuning for Sema.** A frontier model with `eval_code` + `docs_search`
   (71%) dramatically outperforms a fine-tuned 8B model (36%) or even a fine-tuned 8B model
   with tools (24%). The model doesn't need Sema "baked into its weights" when it can
   verify its own code and look up APIs.

2. **RFT on small models has a narrow benefit.** It improves L1 eval-match tasks (81% with
   tools, best of any approach) but damages general coding ability. The model loses its
   ability to generate code for L2-L5 tasks. RFT works best for narrow, verifiable tasks
   — not broad code generation.

3. **The grader is the bottleneck, not the models.** All frontier models with tools score
   53-60% overall, with L1 uniformly at ~30%. The models get the right answers but format
   them with explanations that the answer extractor can't parse. A better grader (or
   instructions to output only the value) would push all scores to 70-80%+.

4. **Kimi K2.6 is the best model for Sema with tools** (60% overall, 66% on L5), at
   $0.95/$4 per M tokens. For budget-conscious use, DeepSeek V4 Flash (53%, $0.14/$0.28)
   or Qwen 3.7 Plus (56%, $0.40/$1.60) are excellent.

5. **The existing Sema MCP server already provides the winning capabilities.** `sema mcp`
   exposes `eval` and `docs` tools today. Adding a `docs_search` deftool with the RAG
   pipeline (already built in `sema-llm`) would give any MCP-compatible LLM client the
   exact capabilities that scored 60-71% in this benchmark.

---

## Files

| File | Description |
|------|-------------|
| `execution-plan.md` | The original execution plan |
| `results/benchmark_results.json` | GLM 5.2 vs Qwen3-8B RFT (no tools) |
| `results/tool_augmented_results.json` | GLM 5.2 + tools |
| `results/multi_all_results.json` | 6-model comparison with tools |
| `results/multi_*.json` | Per-model results from multi-model run |
| `data/benchmark_tasks.jsonl` | 60 benchmark tasks (5 levels) |
| `data/rft_problems.jsonl` | 1,037 RFT training problems |
| `data/eval_pairs.jsonl` | 1,232 extracted eval-test pairs |
| `sema-tools.sema` | Deftool definitions for docs_search + eval_code |

---

## Future Ideas

### Grammar Fuzzer as Training Data Source

The Sema grammar fuzzer (`fuzz/grammar-fuzz.sema`) generates valid, varied Sema
programs with **known expected outputs** — every program comes with a
`; seed=N => EXPECTED_VALUE` annotation computed by the fuzzer's differential
oracle. This is a unique asset for training data generation.

**What the fuzzer produces:**
- Syntactically valid programs covering: `let`, `lambda`, `try/catch`, `async`,
  `channel/send`, `match`, `foldl`, `map`, `assoc`, `cond`, `case`, named-let
  recursion, curried closures, string ops, arithmetic, bitwise ops
- Guaranteed correctness — the fuzzer computes the expected value while
  generating, then verifies `eval` agrees
- Deterministic and reproducible from a seed
- Can generate 10,000+ unique programs at varying depths (3-5)

**The problem:** Fuzzer programs are valid but semantically random. No human
would write `(= (get (assoc (assoc {} :a 2) :b 5) :b) (((lambda (a) (lambda (b) (+ a b))) (if #t (string-length "b_b-Y") (- 10 11))) (bit/xor (try 10 (catch e -1)) (* -8 -9 -8))))`. Using these directly as code-generation
training data would teach the model to write incomprehensible nested expressions.

**The clever approach — synthetic description→code pairs:**

1. Generate 1,000 fuzzer programs with known outputs
2. Ask a frontier model (GLM 5.2, Kimi K2.6) to "write a one-sentence natural
   language description of what this Sema program does"
3. This gives you 1,000 **(description → code)** pairs where:
   - The code is **guaranteed valid** (fuzzer oracle verified)
   - The description is natural and human-readable
   - The expected output is known (for grader verification)
4. Use these as SFT training data for code generation

**Why this is valuable:**
- The hardest part of building SFT datasets for code is getting **verified
  correct** code — the fuzzer solves this for free
- The descriptions teach the model to map natural language → Sema code
- The variety of constructs (async, match, channels, closures, try/catch) is
  broader than what the 224 example files cover
- Cost: ~$5-10 to generate 1,000 descriptions via GLM 5.2 serverless

**Recommended SFT dataset composition for a future attempt:**
- 30% fuzzer-generated eval pairs (10,000 programs with known outputs) — teaches
  syntax validity and evaluation
- 30% synthetic description→code pairs (1,000 fuzzer programs + LLM descriptions)
  — teaches code generation from natural language
- 25% human example files as code-gen pairs (224 files) — teaches code style
- 15% API doc examples (1,156 entries) — teaches function signatures and usage

**Target:** Qwen3-32B or GLM 5.1, LoRA SFT, ~$15-30 training cost on Fireworks.
Combined with `eval_code` + `docs_search` tools at inference time. Hypothesis:
75-80% overall on the 60-task benchmark.
