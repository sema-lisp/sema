# Task 11: Final Profiling, Benchmarking, Tuning, and Release-Readiness Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compare the correctness-complete unified runtime with the frozen
pre-rewrite baseline, explain regressions and improvements across micro and
real-world workloads, tune only justified hotspots, reverify every production
change, run clean confirmation measurements, and write release readiness.

**Architecture:** Baseline and candidate are detached worktrees built with the
same locked toolchain/settings into isolated target directories. One versioned
benchmark corpus drives both where APIs overlap; final-only features receive
absolute measurements. Raw samples, profiles, memory/runtime snapshots,
environment metadata, analysis, and decisions are durable evidence. Benchmark
work starts only after Task 10’s final candidate is green and reviewed.

**Tech Stack:** Hyperfine, Rust/Sema benchmark harnesses, Samply, Playwright/
Chromium, WASM, fake providers/local servers, statistical analysis scripts.

## Execution contract

- **Status:** Ready only after Task 10 is accepted and committed.
- **Dependencies:** Frozen green/reviewed candidate SHA, frozen pre-rewrite
  baseline SHA, complete correctness evidence, and no unresolved finding.
- **Immutable inputs:** Master final benchmark matrix/sequence, correctness and
  maintainability priority, regression policy, reverification requirement, and
  no-release boundary.
- **Exact start state:** Clean worktree; latest commit subject is
  `review(runtime): complete independent correctness campaign`; Task 10 handoff
  records candidate/baseline SHAs and proves no prior profiling affected design.
- **Parallel work:** After harness verification, native common, final-only,
  resource/orchestration, memory, and WASM measurement collectors may run only
  on separate idle machines or sequentially on the one comparison machine; raw
  results never share mutable files. Analysis waits for complete raw data.
  Tuning is serialized per hotspot; confirmation waits for full reverification.

## Global constraints

- Tasks 01–10 must be accepted. Task 10’s complete gate is GREEN and candidate
  SHA is frozen before the first measurement.
- The baseline is the last pre-rewrite production-code commit. The current
  baseline candidate is `3f111e83`; update it only if Task 10 evidence proves
  production code landed before Task 02 began.
- No benchmark result can override correctness, safety, cancellation, cleanup,
  determinism, or maintainability contracts.
- Baseline and candidate use identical benchmark source, fixtures, compiler,
  build flags, browser, hardware/power state, run counts, and sample ordering.
- Existing benchmark suite is retained; runtime work adds coverage rather than
  replacing favorable/unfavorable cases.
- Live LLM/network services are excluded from comparison. FakeProvider and local
  controlled servers make runs repeatable and cost-free.
- Measurement code does not ship in runtime paths. Test-only counters are read,
  not changed to improve a number.
- A production tuning change restarts complete correctness/stress/leak gates and
  all six Task 10 reviews before confirmation benchmarking.
- This task reports release readiness but does not tag, publish, push, deploy, or
  release anything.

---

## Files and responsibilities

**Create**

- `benchmarks/unified-runtime/*.sema` — common CLI/runtime workloads.
- `benchmarks/runtime-host/Cargo.toml` — excluded benchmark harness package.
- `benchmarks/runtime-host/src/bin/common.rs` — stable public embedding API,
  compiled against baseline and candidate.
- `benchmarks/runtime-host/src/bin/final.rs` — final root/drive API and snapshots.
- `benchmarks/fixtures/` — fixed files, SQLite data, HTTP/WS responses, workflow
  journal, MCP messages, FakeProvider scripts, and Sema Coder session inputs.
- `scripts/bench-unified-runtime.sh` — A/B build/run/raw export orchestrator.
- `scripts/analyze-unified-runtime-benchmarks.py` — medians, MAD, percentiles,
  bootstrap confidence intervals, and regression flags.
- `scripts/profile-unified-runtime.sh` — CPU/profile/RSS/retention capture.
- `playground/tests/runtime-benchmark.spec.ts` — WASM heartbeat/latency/memory
  measurement harness, excluded from ordinary pass/fail E2E assertions.
- `docs/plans/evidence/unified-cooperative-runtime/task-11/environment.json`.
- `docs/plans/evidence/unified-cooperative-runtime/task-11/manifest.json`.
- `docs/plans/evidence/unified-cooperative-runtime/task-11/raw/{baseline,candidate,confirmation}/`.
- `docs/plans/evidence/unified-cooperative-runtime/task-11/profiles/{baseline,candidate,tuned}/`.
- `docs/plans/evidence/unified-cooperative-runtime/task-11/analysis.json`.
- `docs/plans/evidence/unified-cooperative-runtime/task-11/summary.md`.
- `docs/plans/evidence/unified-cooperative-runtime/task-11/release-readiness.md`.
- `docs/plans/reviews/unified-cooperative-runtime/task-11-benchmark-review.md`.

**Modify**

- `jake/bench.jake` — runtime A/B, analyze, profile, and confirmation recipes.
- `scripts/bench.sh` — machine-readable environment/sample metadata and strict
  missing-benchmark failure; preserve current suite names/results.
- `docs/performance-roadmap.md` — link results and any explicitly accepted
  post-rewrite performance work.
- Production runtime files only after a profiled hotspot and written tuning
  decision; every such edit follows the reverification loop.

## Exact benchmark manifest

Every manifest row contains `id`, `category`, `source`, `fixture hash`,
`baseline/candidate/final-only`, `warmup`, `samples`, `timeout`, `metrics`, and
`correctness checksum`. The runner rejects a missing fixture, changed hash,
failed checksum, timeout, nonzero exit, or unequal common-workload result.

Required categories and cases:

| Category | Required cases |
| --- | --- |
| Existing VM | all current `scripts/bench.sh --suite all` programs unchanged |
| Root/eval | interpreter construction; empty/small/compiled eval; repeated eval on one interpreter; final-only 1/10/100 concurrent roots |
| Task core | spawn+await chain; spawn batch; yield loop; settlement; explicit cancel; task churn; detached task across roots |
| Fairness | one busy root plus latency probe; 2/10/100 roots; timer/completion wake p50/p95/p99; final-only root fairness distribution |
| Timers | zero/short/mixed timers, insert/cancel churn, drift and wake latency |
| Channels | capacities 1/64/4096, one-to-one, fan-in/out, backpressure, cancelled waiter, close churn |
| Resources | file/stream/SQLite/process/PTY/local HTTP/WS/server throughput, cancellation latency, shutdown/reap |
| Orchestration | FakeProvider concurrent completions, agent tool loop/streaming, MCP queue, workflow fan-out and settled/fail-fast |
| Scenarios | deterministic Sema Coder session, async notebook Run All, local server burst, workflow pipeline |
| Memory/GC | allocations where available, peak/steady RSS, retained runtime counts, cycles, 1,000 interpreter shutdowns |
| WASM | eval throughput, task/timer/channel cases, final-only multiple roots, bundle bytes, WASM memory pages, heartbeat/input/render latency |

Common baseline cases use APIs present at the baseline and avoid relying on the
old implicit-cancelling timeout/race behavior. Final-only APIs such as multiple
host roots, `race-owned`, and `with-timeout` are labeled `final-only`; absence in
the baseline is not reported as an infinite improvement.

The deterministic Sema Coder fixture models a real edit session: load project
metadata, search/read a fixed file tree, run parallel FakeProvider planning and
tool calls, stream output, apply an in-memory patch, run a local command, and
cancel a superseded preview. Its output/file hashes and provider request log are
the correctness checksum.

## Task 1: Implement, validate, and commit the benchmark harness

- [ ] **Step 1: Add correctness-first benchmark tests**

Each workload supports `--verify` and emits a deterministic checksum plus exact
operation count. Test manifest parsing, fixture hash rejection, failed checksum,
timeout, missing sample, baseline/final-only classification, random A/B order,
and partial-run resume without mixing environments.

- [ ] **Step 2: Extend Jake and scripts**

Provide these exact entry points:

```bash
jake bench.runtime-verify
jake bench.runtime-ab baseline=../unified-runtime-bench-baseline \
  candidate=../unified-runtime-bench-candidate samples=30 warmup=5
jake bench.runtime-analyze
jake bench.runtime-profile
jake bench.runtime-confirm
```

- [ ] **Step 3: Verify and commit the harness before timing**

```bash
jake bench.runtime-verify
git add benchmarks scripts jake
git commit -m "perf(runtime): add verified runtime benchmark harness"
```

Expected: every correctness checksum passes; no timing summary is produced by
verify mode. This commit changes measurement code only. It becomes the frozen
candidate SHA so the candidate worktree contains the exact harness being run.

## Task 2: Freeze baseline/candidate and capture environment

- [ ] **Step 1: Read Task 10 handoff and freeze SHAs**

```bash
BASELINE_SHA=3f111e83
CANDIDATE_SHA=$(git rev-parse HEAD)
git merge-base --is-ancestor "$BASELINE_SHA" "$CANDIDATE_SHA"
git status --short
```

Expected: ancestry check succeeds, the candidate includes the verified harness
commit, and the candidate worktree is clean. If Task 10 recorded a replacement
baseline, use it consistently and update this task plan plus evidence before
measuring.

- [ ] **Step 2: Create detached measurement worktrees**

```bash
git worktree add --detach ../unified-runtime-bench-baseline "$BASELINE_SHA"
git worktree add --detach ../unified-runtime-bench-candidate "$CANDIDATE_SHA"
```

Do not reuse development build artifacts. The candidate worktree already
contains the committed benchmark corpus/harness. Copy that exact committed
corpus/harness into the baseline worktree as untracked measurement input and
record identical tree hashes; do not modify baseline production files.

- [ ] **Step 3: Record environment before building**

Capture full SHAs, dirty status, benchmark tree/fixture hashes, `rustc -vV`,
Cargo/LLVM/Node/npm/wasm-pack/Chromium/hyperfine/samply versions, OS/kernel, CPU
model/core count, RAM, power source/mode, thermal state if available, and active
background-process policy in `environment.json`.

## Task 3: Build identical baseline and candidate artifacts

- [ ] **Step 1: Pin build settings**

Use `cargo build --locked --release -p sema-lang`, no PGO, identical explicit
`RUSTFLAGS`, and separate empty `CARGO_TARGET_DIR`s. Build the common host harness
against each worktree with the same source; build final host harness only against
candidate. Build both WASM packages with the same wasm-pack/wasm-bindgen versions
and release settings.

- [ ] **Step 2: Record artifact identity**

Manifest records compiler command, binary/WASM/JS SHA-256, byte size, linked
libraries where available, and build logs. Run `--verify` with the exact binaries
that will be measured.

## Task 4: Run the initial complete A/B campaign

- [ ] **Step 1: Stabilize measurement conditions**

Use the same machine/session, AC power, fixed browser, no concurrent builds,
and documented background-process policy. Warm up each case five times. Run at
least 30 measured samples, randomized in baseline/candidate pairs to reduce
thermal/time drift.

- [ ] **Step 2: Collect time/throughput/latency and correctness**

```bash
jake bench.runtime-ab baseline=../unified-runtime-bench-baseline \
  candidate=../unified-runtime-bench-candidate samples=30 warmup=5
```

Store every raw sample, stdout/stderr/checksum, exit status, order, timestamp,
and artifact hash. Do not retain only aggregate Hyperfine JSON.

- [ ] **Step 3: Collect memory/GC/shutdown and WASM metrics**

Run dedicated repeated scenarios for peak RSS, steady-state RSS after warm-up,
runtime/GC live counts, cleanup plateau, bundle bytes, WASM pages, event-loop
heartbeat, and input/render latency. Use at least 10 long-running samples and 30
latency samples.

- [ ] **Step 4: Analyze without auto-accepting**

```bash
jake bench.runtime-analyze
```

Compute median, MAD, mean/stddev for compatibility, p50/p95/p99 latency,
throughput, peak/steady memory, percent delta, and bootstrap 95% confidence
interval. Flag for investigation when the interval excludes zero and magnitude
exceeds 10%, memory exceeds 15%, tail latency exceeds 20%, behavior is
asymptotically worse, or regressions span three or more categories. Flags start
investigation; they are not automatic release failures or permission to weaken
design.

## Task 5: Profile and explain results

- [ ] **Step 1: Profile representative baseline/candidate cases**

```bash
jake bench.runtime-profile
```

Capture Samply profiles for root/eval, task churn, multi-root fairness, channel
contention, timer storm, local HTTP, FakeProvider agent, Sema Coder, and shutdown.
Capture allocation/RSS/retention and browser performance traces where supported.
Compress raw profiles losslessly and record tool/version/load commands.

- [ ] **Step 2: Attribute every flagged regression and major improvement**

For each flag, identify workload, magnitude/confidence, profile frames/allocation
source, complexity explanation, correctness/maintainability relationship, and
decision: measurement artifact, expected justified cost, benchmark defect, or
tuning candidate. “Runtime rewrite overhead” is not sufficient attribution.

- [ ] **Step 3: Have an independent benchmark reviewer inspect methodology**

Finding IDs use `UR-T11-R###`. Reviewer checks baseline choice, source/fixture
identity, build parity, randomization, sample sufficiency, checksums, statistical
analysis, profiles, final-only labeling, and interpretation. Fix methodology
findings and rerun affected initial measurements before tuning.

## Task 6: Tune only justified hotspots

- [ ] **Step 1: Write a tuning decision before each production edit**

Record profile evidence, proposed local change, invariants at risk, regression
tests, expected measurement, and rejection condition. Prefer simpler data layout,
fewer allocations, or bounded batching. Do not restore TLS scheduler ownership,
nested driving, untraced state, blocking paths, implicit cancellation, unbounded
resources, replay, or coarse unfair quanta.

- [ ] **Step 2: Implement test-first and measure the isolated case**

Run affected correctness tests before/after, then a short exploratory benchmark.
Discard a tuning change if benefit is noise, merely shifts cost to tail/memory,
or harms clarity disproportionately.

- [ ] **Step 3: Reverify all production tuning**

After the final production edit, rerun Task 09’s complete correctness,
adversarial, fuzz-seed, leak, watchdog, browser, and package gate. Then repeat
all six Task 10 review rounds against the tuning diff and independently close
their addenda. Freeze a new reviewed candidate SHA. No confirmation measurement
starts earlier.

## Task 7: Run clean confirmation benchmarks

- [ ] **Step 1: Create a fresh detached tuned worktree and empty targets**

Use the frozen reviewed SHA, the same baseline SHA, benchmark corpus, compiler,
flags, fixtures, machine policy, warmups, samples, and randomized A/B pairing.

- [ ] **Step 2: Run full confirmation, not only tuned cases**

```bash
jake bench.runtime-confirm
```

Store raw results separately under `raw/confirmation`; never overwrite initial
data. Recompute complete analysis and verify correctness checksums.

- [ ] **Step 3: Explain differences from initial campaign**

Summary distinguishes implementation tuning from measurement noise/environment
change and reports any new regression introduced outside the tuned workload.

## Task 8: Write release readiness and commit evidence

- [ ] **Step 1: Write `summary.md`**

Include baseline/final SHAs, method, matrix coverage, key results, flagged cases,
profiles, tuning attempts kept/rejected, justified regressions, improvements,
limitations, and reproducible commands. Link raw files rather than pasting only
selected numbers.

- [ ] **Step 2: Write `release-readiness.md`**

Confirm Tasks 01–10, post-tuning correctness/reviews, confirmation benchmarks,
static deletion, docs/assets/package gates, and unresolved findings. The verdict
is `ready for release procedure` only when there is no unresolved correctness,
safety, security, leak, determinism, shutdown, maintainability, broad/severe/
asymptotic, or unexplained performance blocker. Minor understood regressions may
be accepted with explicit rationale.

- [ ] **Step 3: Final independent evidence review**

Reviewer checks raw-to-summary traceability, reruns a representative A/B subset,
and confirms release-readiness claims do not exceed evidence.

- [ ] **Step 4: Commit without releasing**

```bash
git add benchmarks scripts jake docs/performance-roadmap.md \
  docs/plans/evidence/unified-cooperative-runtime/task-11 \
  docs/plans/reviews/unified-cooperative-runtime/task-11-benchmark-review.md \
  crates playground Cargo.toml Cargo.lock
git commit -m "perf(runtime): record final runtime benchmark campaign"
```

Do not tag, push, publish, deploy, or invoke the release recipe.

## Completion criteria

- Baseline and final candidate are correct frozen SHAs with identical build and
  measurement conditions.
- Existing, runtime micro, resource, orchestration, Sema Coder, notebook/server/
  workflow, memory/GC/shutdown, and WASM cases are covered.
- Raw samples, checksums, metadata, profiles, analysis, and decisions are durable
  and independently reviewable.
- Every broad/severe/asymptotic/unexplained regression was investigated and
  received a justified tuning attempt without weakening architecture.
- Any production tuning passed the complete correctness/adversarial/leak/browser/
  package gate and all six review rounds again.
- Clean full-matrix confirmation results exist on the reverified SHA.
- Release-readiness has no unresolved blocker and no release action was taken.
