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

Benchmark sources: six `.sema` programs (spawn-storm, deep-await, sleep-storm,
primes, cons-1m, channel-pingpong) — reproduce with
`hyperfine --warmup 2 --runs 7 '<binary> <prog>.sema'` against a
`jake wt-new`-built baseline at `3f111e83`. Bisect oracle: primes
median-of-5 < 38ms.
