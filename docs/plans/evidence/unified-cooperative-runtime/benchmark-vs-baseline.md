# Benchmark vs pinned baseline `3f111e83` — REGRESSION FOUND

Date: 2026-07-16. Branch: `codex/unified-async-runtime` (at beb6108d).
Method: `hyperfine --warmup 2 --runs 7`, release builds (fat LTO, cgu=1) of both
trees, macOS arm64, identical benchmark programs verified to produce identical
output on both binaries. This supersedes the P8 report's qualitative "no
regression" finding, which never ran a baseline A/B (it profiled the current
tree in isolation).

## Results (mean ± σ)

| benchmark | baseline | current | ratio | shape |
|---|---|---|---|---|
| spawn-storm (2000 spawn+await) | 37.0 ± 4.4 ms | 36.2 ± 2.7 ms | **1.00×** | task machinery |
| deep-await (depth 300) | 14.1 ± 2.7 ms | 18.2 ± 3.0 ms | 1.29× | task machinery |
| sleep-storm (500 × 1ms) | 17.1 ± 2.3 ms | 36.7 ± 3.6 ms | 2.15× | timer wheel |
| primes n<10000 (pure VM + `filter` HOF) | 27.9 ± 4.4 ms | 54.7 ± 4.6 ms | **1.96×** | HOF callback |
| cons-1m (pure VM loop, no HOF) | 81.7 ± 4.7 ms | 110.0 ± 7.9 ms | 1.35× | dispatch loop |
| channel-pingpong (20k rendezvous msgs) | 33.5 ± 1.4 ms | 248.8 ± 22.8 ms | **7.43×** | channel wait/wake |

## Root cause — bisected, not inferred

Automated `git bisect run` (oracle: primes median-of-5, threshold 38ms; all
steps well-separated at 23–24ms vs 45–53ms) lands on:

**`30537e03` — feat(eval): make the unified runtime the sole async engine
(universal flip).**

Every pre-flip commit measures at baseline speed *including all the cooperative
machinery commits* (structural ABI `23209414^`, promise/channel registries,
cooperative HOFs). The machinery is not slow when dormant — the flip made it
the universal path:

- **HOF callbacks** (`filter`/`map`/…): inside a runtime quantum every element
  callback takes the cooperative `NativeOutcome::Call` continuation path
  (park parent VM → dispatch callback VM → reinstall) instead of the direct
  nested `run_inner` call. ~2.7 µs/element overhead → primes' 2×.
- **Channel rendezvous**: each send/recv parks the task and completes through a
  full drive-loop turn (task-map remove/insert per quantum, barrier re-check per
  turn). 12.5 µs/message vs 1.7 µs baseline → pingpong's 7.4×.
- **Dispatch loop accounting**: the per-instruction budget check costs ~13% on
  compute-bound code (isolated by experiment: infinite-limit build 54.7→47.6ms;
  `budget=None` build 46.3ms — the residual vs 27.9 is the HOF path above).
  Quantum swaps themselves are noise at the default 1M-instruction limit.
- **Task machinery is NOT regressed**: spawn-storm at parity, deep-await +29%
  (absolute 4ms over 300 chained spawn+await, i.e. ~13µs/task lifecycle —
  acceptable).

## Verdict

The >10% gate (orchestration plan, Slice 0) trips: proceed-with-fix. Tracked as
**Slice 0b: runtime fast-path performance recovery** targeting (in order of
leverage): (1) channel ready-peer fast path that settles a rendezvous without a
full park/drive-turn round trip, (2) HOF callback dispatch without task-map
churn (reuse the parked-parent slot across elements), (3) batched instruction
accounting (register-local countdown, resync at block boundaries), (4) timer
wheel wake batching (sleep-storm). Re-run this exact suite after each fix;
program exit requires ≤1.10× on every row except where a documented semantic
trade-off is accepted.

## Post-bisect verification note (same day)

A profiling pass initially measured instruction parity on primes and disputed
the 2× — traced to a stale binary: the bisect had left a post-flip build in the
baseline worktree's `target/` (`git bisect reset` restores source, not
artifacts). With the baseline **rebuilt at `3f111e83` and verified**, the
regression is instruction-backed: primes 256.8M → 540.0M instructions retired
(2.10×), pingpong 6.5× wall on re-run. Binary identity is now part of the
measurement protocol (see the Slice 0b plan). The per-element cost is ~28k
instructions per HOF callback element.

Benchmark sources: six `.sema` programs (spawn-storm, deep-await, sleep-storm,
primes, cons-1m, channel-pingpong) — reproduce with
`hyperfine --warmup 2 --runs 7 '<binary> <prog>.sema'` against a
`jake wt-new`-built baseline at `3f111e83`. Bisect oracle: primes
median-of-5 < 38ms.

## Slice 0b close-out (2026-07-16, post Tasks A–E, HEAD ec4c4495)

Final matrix, same protocol, baseline binary identity re-verified:

| benchmark | baseline | after 0b | ratio | was |
|---|---|---|---|---|
| spawn-storm | 32.5 ms | **21.8 ms** | **0.67× (faster)** | 1.00× |
| primes (HOF) | 31.8 ms | **23.6 ms** | **0.74× (faster)** | 1.96× |
| cons-1m | 79.0 ms | 108.8 ms | 1.38× | 1.35× |
| sleep-storm | 16.7 ms | 27.7 ms | 1.65× | 2.15× |
| deep-await | 11.7 ms | 20.6 ms (σ 12.9 — noisy) | ~1.7× | 1.29× |
| channel-pingpong | 32.4 ms | 91.3 ms | **2.82×** | 7.43× |

Instruction-count oracles (low-noise): primes 540M→280M (baseline 257M);
pingpong 2.50B→1.18B (baseline ~400M — the earlier 1.23B "baseline" figure was
stale-binary-contaminated and is corrected here). IPC is equal across binaries;
wall now tracks instructions.

**What landed:** A drive-loop clock batching (~47% of drive samples were
Instant::now), B register-local instruction countdown, C in-place HOF callback
dispatch on a reused scratch VM (the big one — primes now beats baseline),
D matched-rendezvous inline completion, E empty-scope seam-swap skip. Every
task adversarially reviewed (Opus); two bugs caught pre-merge (per-element
upvalue snapshot; continuation resume under a live RuntimeState borrow that
starved GC).

**Residuals (all >1.10×, per-row explanation):**
- *pingpong 2.82×* — the remaining ~19k instr/message is the genuine-park half
  of each capacity-1 rendezvous: quantum park/unpark with `Box<VM>` moves and
  task-map churn. Closing it needs direct task-to-task handoff (peer's resume
  value written without parking the sender) — a structural follow-up, not a
  tweak.
- *sleep-storm 1.65× / deep-await ~1.7×* — per-task lifecycle overhead
  (spawn+timer+settle through the drive loop); absolute deltas are ~10 ms per
  500 tasks. spawn-storm (same machinery, no timers) is FASTER than baseline,
  so the residual is timer-wheel + park path specific.
- *cons-1m 1.38×* — NOT explained by any 0b target (budget check removed,
  no HOF, no channels); suspected allocator/GC-registry interaction under the
  runtime. Needs its own diagnosis; pre-existing vector-cons O(n) shape makes
  this benchmark allocation-bound.

**Verdict:** the two workloads users hit most (HOF-heavy compute, task
fan-out) now beat the pre-migration engine. The ≤1.10× bar is NOT met on
4 of 6 rows; residuals are characterized with named structural follow-ups.
Accept-or-continue is an owner decision recorded in the orchestration plan.

**Owner decision (Helge, 2026-07-17): residuals ACCEPTED.** Slice 0b closes;
the program proceeds to P6-1. The three structural follow-ups (direct
rendezvous handoff, timer/park lifecycle, cons-1m allocator diagnosis) are
initially parked, then (same day) redirected into an immediate deeper pass:
Slice 0c — symbolized profiling, divan/criterion scheduler micro-benchmarks,
targeted squeezes. Tracked as PERF-RESIDUAL-1 in docs/deferred.md.

## Micro-benchmark reference (0c-3)

Date: 2026-07-17. Branch: `codex/unified-async-runtime`. New divan suite,
`crates/sema-vm/benches/runtime_micro.rs`, isolating the scheduler's hot
primitives at native-op granularity (µs–ns, vs. the hyperfine suite's
whole-program ms). Every Sema source form is read + compiled once, outside
the timed closure; each iteration drives a fresh `VM` (or, for the
shutdown/idle-with-parked-tasks benches, a fresh `Interpreter`) through the
real `Runtime` — `sema-vm`'s public API only, nothing under
`crates/sema-vm/src/` was touched to build it. Reproduce with:

```
cargo bench -p sema-vm --bench runtime_micro
# or: jake bench.micro
```

| benchmark | fastest | slowest | median | mean | samples | iters |
|---|---|---|---|---|---|---|
| idle_drive_turn | 71.3 ns | 77.18 ns | 73.27 ns | 73.51 ns | 100 | 6400 |
| spawn_settle | 687.1 ns | 765.3 ns | 718.4 ns | 721 ns | 100 | 800 |
| timer_arm_and_fire | 1.624 µs | 6.541 µs | 1.708 µs | 1.775 µs | 100 | 100 |
| channel_rendezvous | 4.874 µs | 299.4 µs | 5.083 µs | 8.195 µs | 100 | 100 |
| hof_map_100 (100 elements) | 9.749 µs | 65.62 µs | 9.999 µs | 10.65 µs | 100 | 100 |
| idle_turn_with_parked_tasks(n=0) | 200.8 µs | 522.2 µs | 208 µs | 213.4 µs | 100 | 100 |
| idle_turn_with_parked_tasks(n=64) | 311.2 µs | 520.4 µs | 326.4 µs | 332.1 µs | 100 | 100 |
| idle_turn_with_parked_tasks(n=256) | 826.5 µs | 1.078 ms | 865.6 µs | 881.6 µs | 100 | 100 |
| shutdown_sweep(n=0) | 201.6 µs | 471.8 µs | 206.6 µs | 215.7 µs | 100 | 100 |
| shutdown_sweep(n=64) | 312.3 µs | 1.383 ms | 328.2 µs | 368.4 µs | 100 | 100 |

Notes:
- `hof_map_100` ≈ 100 ns/element for the cooperative `NativeOutcome::Call`
  dispatch path (`map` over a 100-element list, trivial `(fn (x) (+ x 1))`),
  consistent with the ~2.7 µs/element figure quoted in the bisect section
  above being dominated by allocation/GC-adjacent costs at whole-program
  scale rather than the dispatch primitive itself.
- `shutdown_sweep`'s n=0 vs n=64 median delta (~122 µs) is the cost of
  `Runtime::shutdown` fanning cancellation out to, and draining, 64 parked
  tasks via the private `cancel_waiting` scan — genuinely O(N) here because
  shutdown queues every live task onto `pending_cancel_waits`, so each must
  be visited at least once. n=0's ~207 µs floor is `Interpreter::new()` +
  `Runtime::shutdown` overhead common to both rows.
- `idle_turn_with_parked_tasks` times ONE `Runtime::drive` turn against a
  runtime already holding N *uncancelled* parked tasks (no shutdown, no
  cancellation requested) — the case `shutdown_sweep` cannot isolate, since
  its cost is dominated by VM drops/executor teardown and (per the note
  above) a real O(N) cancellation fan-out. `cancel_waiting` itself is O(1) in
  this uncancelled case (`pending_cancel_waits` is empty, so the scan's
  `attempts` bound is 0 and it returns immediately) — inspected directly in
  `crates/sema-vm/src/runtime/state.rs`. The measured turn is NOT flat,
  though: median cost grows from 208 µs (n=0) to 326 µs (n=64) to 866 µs
  (n=256), roughly linear in N. That growth is not evidence against the 0c-2
  fast path — it traces to a separate, unconditional O(N) cost every
  `Runtime::drive` turn already pays regardless of cancellation state: the
  `ready_remaining` computation (`state.tasks.values().any(...)` in
  `runtime/state.rs`), which scans every tracked task once per turn to
  answer "is anything else ready?". This is a real per-turn cost worth
  tracking as its own regression reference, but it is orthogonal to what
  this bench was added to isolate; it is flagged here rather than
  mischaracterized as a `cancel_waiting` regression.
- Both `shutdown_sweep(n=64)` and `idle_turn_with_parked_tasks` rows show a
  long tail (max up to 1.4 ms) — shared with a machine under load during
  this run; the median/mean-of-100 figures are the load-bearing numbers, not
  the single worst sample.
- This suite is a µs/ns-granularity *regression reference* for the scheduler
  primitives in isolation; it complements, and does not replace, the
  whole-program hyperfine matrix above.

## Slice 0c close-out (2026-07-17, HEAD 1a997d46)

Final matrix (same protocol; baseline binary identity verified):

| benchmark | baseline | after 0c | ratio | 0b ratio | flip-era |
|---|---|---|---|---|---|
| spawn-storm | 40.7 ms | **27.2 ms** | **0.67× (faster)** | 0.67× | 1.00× |
| sleep-storm | 16.9 ms | **14.8 ms** | **0.88× (faster)** | 1.65× | 2.15× |
| primes | 22.4 ms | 24.4 ms | 1.09× ✅ | 0.74× | 1.96× |
| cons-1m | 78.3 ms | 80.8 ms | 1.03× ✅ | 1.38× | 1.35× |
| deep-await | 11.4 ms | 12.6 ms | 1.11× ✅(σ) | ~1.7× | 1.29× |
| channel-pingpong | 33.9 ms | 66.6 ms | **1.97×** | 2.82× | 7.43× |

Instructions retired: primes 277M (baseline 257M), pingpong 880M (baseline
~400M), cons-1m 1.41B.

**What landed in 0c:** hashbrown for id-keyed runtime maps (SipHash was ~25%
of pingpong); O(1) `cancel_waiting` via a pending-cancellation queue (was a
per-rotation full-task scan — top sema fn in deep-await); divan micro-benchmark
suite (`jake bench.micro`) as the go-forward regression reference; completion-
inbox polling gated behind an atomic dirty flag (lost-wakeup-safe: the blocking
inbox path is an ungated correctness floor); fire_timer clock consolidation;
depth-bounded recursive drop with worklist spill (cons-1m 1.38×→1.03×);
O(1) `ready_remaining` from ready-queue membership (debug_assert-pinned
turn-boundary equivalence). All Opus-reviewed; drop path Miri-clean.

**Remaining residual: channel-pingpong 1.97× (~12k instr/message).** The
genuine-park half of each capacity-1 rendezvous still pays quantum park/unpark
(`Box<VM>` + memmove visible in the profile). The named follow-up stands:
direct task-to-task handoff. All other PERF-RESIDUAL-1 rows are RESOLVED.

**Known bench-quality gap:** `idle_turn_with_parked_tasks` (divan) is
setup-dominated (n=0 ≈ 207 µs vs 84 ns for `idle_drive_turn`) and cannot yet
prove per-turn flatness in parked-task count; the `ready_remaining` fix is
instead pinned by its debug_assert. Rework the bench when the suite is next
touched.

## Task 0c-7 close-out — direct rendezvous handoff (2026-07-17, HEAD e3285778)

A channel op whose match is immediately available now completes without parking
its own task: the registry is consulted before the VM is boxed; the response is
applied to the still-unboxed VM which re-enters `run_quantum` with the
remaining budget; the peer rides the pre-existing inline delivery. Adversarial
Opus review: SAFE on all seven surfaces (FIFO — the mixed queue state was
proven unreachable from registry invariants; budget continuation; three
cancellation windows; applied-value/park boundary; barrier/fairness; GC/borrow
discipline). Both pinned tests carry demonstrated teeth (each fails when its
property is deliberately broken; the budget test measures drive-turn chunking
of a 50k-send spinning loop). `ChannelHandoffOutcome::Deferred` proven
unreachable from Sema source (production channel continuations never compose
further); pinned via a synthetic continuation unit test.

| benchmark | ratio (wall) | note |
|---|---|---|
| channel-pingpong | **~1.36–1.4×** (565M vs ~400M instr) | was 1.97× pre-handoff, 7.43× at flip |
| spawn-storm | 0.58× (faster) | |
| sleep-storm | 0.89× (faster) | |
| primes / cons-1m / deep-await | 1.1–1.2× band | run-to-run ratio wobble ±0.1; instruction oracles unchanged |

divan `channel_rendezvous`: 5.25 µs → 4.50 µs median.

**Remaining pingpong residual (~4k instr/message over baseline)** is per-quantum
overhead on the genuinely-parked half (TaskScopeSwap probes, task-id publish,
quantum guard) — diffuse, no single lever left. The squeeze pass ends here;
0.58×–1.4× across the matrix vs the pre-migration engine is the recorded
end-state.

## Pre-merge-to-main A/B — `jake bench` VM suite regresses 3–5× (2026-07-24)

Candidate `e0e5acb8` (main `14c44309` + `origin/codex/unified-async-runtime`
merged) vs baseline main `14c44309`. Method: `jake bench.save` (release, fat
LTO, cgu=1, mode=vm, 10 runs + 3 warmup), macOS arm64; min-time (tightest,
contention-robust) reported with mean cross-checked. This suite is the standard
whole-program compute benchmark set (`examples/benchmarks/*.sema`) — distinct
from and complementary to the async-focused hyperfine matrix above, which never
included these programs.

| program | baseline min | candidate min | ratio | σ (base→cand) |
|---|---|---|---|---|
| deriv | 0.645 s | 3.081 s | **4.78×** | 0.007→0.036 |
| string-pipeline | 0.541 s | 2.122 s | **3.92×** | 0.014→0.011 |
| higher-order-fold | 0.535 s | 1.740 s | **3.25×** | 0.072→0.048 |
| mandelbrot | 0.153 s | 0.175 s | 1.15× | tight |
| hashmap-bench | 3.219 s | 3.550 s | 1.10× | tight |
| tak / nqueens / closure-storm / throw-catch / upvalue-counter | — | — | ≤1.06× | noise |

`deriv` and `higher-order-fold` are byte-identical between baseline and
candidate; `string-pipeline`'s only diff is let-binding whitespace. Self-timed
program clocks confirm the harness (`deriv` prints 3066 ms; `higher-order-fold`
foldl-part 743 ms + map-part 270 ms — `map`/`foldl` per-element cost is
comparable, so the regression is the universal-flip's per-op + allocation/GC
overhead on large-list-building loops, NOT foldl-specific). This is the cons-1m
allocator/GC-registry suspect (PERF-RESIDUAL-1) at higher allocation scale, and
it was outside the accepted-residuals decision. **Merge-innocent** (branch-tip
hot path == candidate hot path; profile identical; main un-diverged from fork
`3f111e83`). **Release gate**: tracked in `docs/deferred.md` PERF-RESIDUAL-1
(REOPENED). Reproduce: `jake bench.save` on each side; compare
`target/bench/bench-<sha>.json` min fields.
