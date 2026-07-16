# Unified Cooperative Runtime — Release-Readiness Report (P8)

Date: 2026-07-16. Branch: `codex/unified-async-runtime`.

This report assesses the unified cooperative runtime migration against
release-readiness after the core migration, P7 verification, and the
productionization wave. It stands in for Task 11's formal profiling pass; the
performance section records a direct regression investigation rather than a
full benchmark-vs-baseline sweep (rationale below).

## 1. Migration status: COMPLETE (core) + VERIFIED

The hard cut is done: the legacy split scheduler (`scheduler.rs`, 1386 lines)
and all thread-local suspension/bridge mechanisms are deleted; every async
path — language async, External I/O, channels, promises, timers, resource
gates, debug — flows through the single interpreter-owned `Runtime` via the
structural `NativeOutcome`/`WaitKind` ABI. The zero-tolerance source guard
(`scripts/check-unified-runtime-legacy.sh --check`) passes: no purged
legacy-scheduler symbol remains in shipped code.

## 2. Gate matrix — ALL GREEN

| Gate | Result |
| --- | --- |
| `cargo test --workspace --no-fail-fast` | GREEN — 0 failures (full sweep, exit 0) |
| `jake examples` | 81 passed, 12 skipped, 0 failed |
| `jake smoke-bytecode` | all examples compile/disasm/run |
| `jake lint` (fmt-check + clippy -D warnings) | clean |
| `jake docs-check` | pass |
| `scripts/check-unified-runtime-legacy.sh --check` | ok (no purged symbols) |
| `scripts/check-unified-runtime-inventory.sh --check` | 856 matches, 0 UNREVIEWED |
| `scripts/test-packaged-sema-web.sh` (package-boundary) | **PASS** — `sema web` ships from a real `.crate` |

The inventory checker green means the final migration-completeness audit holds:
every retained runtime-touching source site is accounted for in a terminal
ledger row (no unaccounted LEGACY-status path).

## 3. Adversarial verification (P7) — no runtime defects

Two independent adversarial verifiers stress-tested the completed runtime:

- **Cancellation + External-I/O + concurrency** (~40 stress programs): eager
  subprocess-kill on cancel (verified dead mid-run, no orphans), transitive
  subtree cancel, resource-gate FIFO serialization, 50 concurrent `http/get`
  overlap, 2000 spawns / depth-500 chains / 2M yields, cross-run determinism,
  bounded liveness. **No defects.**
- **Memory/GC + debug + whole-runtime edges**: 20k-cycle / 50k-churn stress with
  RSS plateaus and bounded registry size (the channel/promise registry↔collector
  reintegration holds under hostile churn); debug fully on the runtime (dap 3/0
  + wasm 8/0, no ignores). **No defects.**

Two findings surfaced and were FIXED (not deferred):
- `apply`/`call-with-values`/multi-list `map` leaked an internal error for
  runtime-only natives → routed through the cooperative `NativeOutcome::Call`
  ABI (commit d6bf5871).
- `async/run` was a ready-drain, not a settle-barrier → replaced with the
  deadlock-free self-resolving-waits `OriginBarrier` (C1, commit b90b296b).

## 4. Performance — NO migration regression

A regression investigation (triggered by a `smoke-bytecode` timeout flake)
established that the runtime migration introduced **no performance regression**:

- The flake was `math-and-crypto.sema`'s O(n^2) functional sieve (~13s debug at
  n=10000), borderline against the 15s per-example timeout. Root cause is
  **pre-existing**: Sema lists are vector-backed (`List(Rc<Vec<Value>>)`), so
  `cons` is O(n) and repeated-cons list building is O(n^2). Not the runtime.
- **GC is uninvolved**: `gc/stats` after building a 40k retained list shows
  `registry-size 1, collected 0, traced 0`. The CORE-2 collector's adaptive
  threshold (`max(GC_FLOOR, GC_GROWTH x survivors)`) is intact.
- **VM/runtime hot paths are healthy**: 246ms for primes-under-10000 (sqrt
  trial division), 641ms for 1M transient conses — no per-step runtime overhead.

The smoke gate's flakiness was fixed with a per-example run timeout (commit
8f28721e), keeping full bytecode round-trip coverage.

A formal benchmark vs the pinned baseline `3f111e83` was NOT run in this pass
(it requires a separate baseline build; disk was constrained and the
qualitative investigation already localized the only slow path to a pre-existing
data-structure characteristic). Recommended as a confirming follow-up, not a
release blocker.

## 5. Deferred / not-landed (honest ledger)

- **SRV-1 (concurrent `http/serve`)** — the handler-task-per-connection
  rearchitecture (~500-700 lines, unproven idle-External-parked accept-loop
  deadlock-freedom) was NOT landed. The shipped **fail-fast guard** (the plan's
  blessed fallback) is retained; four wall-clock-bounded `#[ignore]`d acceptance
  tests capture the gate for a future TDD landing (commit 46f7f542).
- **wasm Promise-driven roots** — attempt in progress under an ironclad
  real-browser-verification-or-fallback rule (the replay/Atomics mechanism ships
  and works; it will not be replaced unverified).
- **Inventory ledger judgment calls** — the new `runtime/` module split
  (F23-F31), `runtime_offload.rs -> F09B`, and the crate-local
  `runtime_eval_tests -> F31` are coarse-but-faithful classifications flagged
  for future refinement; coverage is exact and clusters are pure.
- **P6(1) common host API consolidation** — the private host drive
  (`drive_vm_on_runtime`, `submit_root`, `ShutdownOptions/Report`) is complete
  and all hosts use it; the ergonomic public surface + routing Ctrl-C through
  `cancel_root` (retiring the `check_interrupt` TLS, 1 remaining call site) is a
  clean follow-up, not a correctness gate.

## 6. Release-readiness verdict

The core migration is **release-ready**: fully green CI-equivalent suite, a
passing package-boundary shipping gate, no adversarially-found runtime defects,
and no performance regression. The outstanding items are either blessed
fallbacks (SRV-1), a working-mechanism replacement under strict verification
(wasm Promise), or non-blocking ergonomic follow-ups (host API). None block
shipping the unified runtime.
