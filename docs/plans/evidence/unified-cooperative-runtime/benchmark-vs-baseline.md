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
parked as a deliberate later optimization pass — tracked as PERF-RESIDUAL-1 in
docs/deferred.md.
