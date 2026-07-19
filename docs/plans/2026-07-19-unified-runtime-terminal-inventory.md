# Unified Runtime Terminal Inventory

**Status:** In progress 2026-07-19

## Goal

Make every async-runtime inventory row honestly terminal. Do not relabel a
retained blocking, replay, callback-re-entry, or unbounded path to make the
checker green. A row reaches `MIGRATED`, `REMOVED`, or `SYNCHRONOUS-PROOF` only
after its implementation, cancellation/ownership contract, regression, and
source guard agree.

The post-finish audit classified 109 of 132 rows as already terminal and found
23 blockers:

- `F08B F09A F09B F10 F14 F21A F29 F32 F35A F35B`
- `R17C R18B R23A`
- `C07C C07D C10`
- `H10B H10C H13`
- `V02 V05 V06 V09`

## Constraints

- Preserve one interpreter-owned runtime. A compatibility API may block a host
  thread only when a runtime quantum is provably inactive.
- Runtime tasks and roots express waits through `NativeOutcome` and `WaitKind`;
  no thread sleep, generic `block_on`, whole-program replay, or synchronous
  evaluator callback is permitted inside an active quantum.
- Preserve CORE-2 invariant I2. Continuations trace every retained `Value`; host
  infrastructure captures `Weak` handles or send-only payloads.
- Cancellation removes the exact waiter immediately and invokes resource
  teardown once. Quarantined work has a pre-dispatch cap and cleanup deadline.
- Shipped WASM and `sema web` assets are regenerated from tracked inputs and
  verified at the package boundary.

## Task 1: Finish terminal resource ownership

Complete the in-flight `ResourceGateHandle` remediation for streams, SQLite,
KV, process, PTY, serial, and MCP. Same-runtime close remains structural;
foreign-runtime close uses the owner capability and offloads terminal process,
PTY, and MCP waits without blocking the caller VM.

Required regressions:

- used stream drop returns the owner gate count to baseline;
- every mapped resource closes from another interpreter without touching the
  caller's gate count;
- proc, PTY, and MCP foreign close leave a sibling runnable;
- terminal continuations propagate `Failed`, cancellation, and unexpected
  runtime responses.

## Task 2: Enforce the host-only blocking boundary

Make `blocking_sleep_ms` and generic `io_block_on` reject use from an active
runtime quantum. Retain them only as explicit host/plain-worker adapters.

Migrate every runtime LLM root and task path:

- native provider work parks on External waits even for the root main task;
- retry and rate-limit delays park on Timer waits or run inside an offloaded
  future/worker;
- a fallback chain containing a Sema-defined provider never reaches a native
  provider's `io_block_on` on the VM thread;
- cancellation during provider, retry, and pacing waits is immediate and does
  not charge usage for an unissued request.

Plant tests must first prove the current root blocks a sibling during a delayed
provider/retry/rate-limit path and that `io_block_on`/`blocking_sleep_ms` are
currently callable under `RuntimeQuantumGuard`. Tooth mutations replace the
structural wait with the old blocking call and must fail.

Rows closed: `F08B F09B F35B C07D C10`. Rewrite `F09A/F35A` around the retained
executor's actual role as the runtime's host/background service, with exact
runtime-boundary guards and abort/shutdown tests; do not claim the public seam
was deleted if it remains.

## Task 3: Remove active-task synchronous evaluator re-entry

Inventory every production `call_callback`, `with_stdlib_ctx`, `CURRENT_VM`, and
foreign-VM fallback. Any callback reachable from a live runtime root/task must
return `NativeOutcome::Call` and park its caller while the callback runs.

Migrate at least:

- `context/with` and signal callbacks;
- Sema-defined LLM provider completion/stream callbacks;
- LLM predicate, validation, body, pmap/filter, tool-event, and handler calls;
- workflow/context/OTel callback sites still reachable in a quantum.

After the last active-task reader is gone, remove the `CURRENT_VM` raw-pointer
stack, task-time `STDLIB_CTX` fallback, `QuantumSuspendGuard`, and foreign
synchronous runtime bridge. If a synchronous host-only callback remains, give
it an explicit entry point guarded by `!in_runtime_quantum()` and document its
proof row separately.

Regressions cover suspension, failure, cancellation, captured-cell mutation,
two interpreters with colliding local IDs, and CORE-2 collection for every
callback family. Source guards prevent reintroduction.

Rows closed: `F10 F14 F29 F32 R18B R23A C07C`.

## Task 4: Own cassette scope explicitly

Move MCP and LLM cassette selection out of ambient TLS-only ownership into the
task/scope context captured at spawn and restored per quantum. Concurrent
siblings and interpreters must record/replay against their own cassette, and
cancellation/drop must restore the displaced scope.

Rows closed: `F21A` plus the cassette portion of `C07C`.

## Task 5: Bound stream aggregation

Give `stream/read-all` and `stream/copy` a hard byte cap captured before
dispatch and enforced before every buffer growth/write. File-to-file copies
must either use ordered dual-resource acquisition with interruptible chunked
copy, or fail fast in a runtime quantum with a bounded-chunk guidance message;
they may not fall through to a VM-thread EOF loop. Stdin aggregation/copy must
have an interruptible resource/wake path; a cleanup deadline alone does not make
an open stdin read bounded.

Regressions cover exact-boundary success, one-byte-over rejection without excess
allocation, cancellation while stdin remains open, sibling progress, worker and
gate counts returning to baseline, and the synchronous compatibility path.

Row closed: `R17C`; proof row `V02` becomes terminal.

## Task 6: Remove WASM debugger replay and sync worker blocking

Drive debugger HTTP through a Promise-owned root that resumes the same VM/task
after fetch. Preserve breakpoint/step/locals inspection between turns without
restarting program execution. Delete `HTTP_AWAIT_MARKER`, the debug HTTP cache,
restart arm, and replay retry machinery.

Remove synchronous XHR and Atomics sleep compatibility. A synchronous WASM API
that encounters a suspension must fail promptly with an actionable
`evalPromise`/Promise-entry hint; every supported async entry, including bytecode
archive execution, gets a Promise/root equivalent. No main-thread busy polling
is permitted.

Browser regressions prove:

- a side effect before debug HTTP executes exactly once across fetch,
  breakpoint, continue, and completion;
- cancelling a debug fetch/timer settles only that interpreter's root;
- sync entry points fail promptly on suspension without XHR, Atomics, or spin;
- two interpreters retain isolated debugger/output/root state.

Regenerate the tracked playground and embedded `sema web` assets only after the
source guards are green. Verify byte-identical generated copies, valid WASM, the
focused browser job, and the real packaged `.crate`.

Rows closed: `H10B H10C H13 V05 V09`.

## Task 7: Make proof executable and reconcile evidence

Extend the production checker with comment-stripped, exact allowlists for every
retained host/plain-worker blocking adapter. Add mutation fixtures for
`io_block_on`, `blocking_sleep_ms`, synchronous callback re-entry, uncapped EOF
aggregation, HTTP markers/cache replay, XHR, and Atomics.

Then:

1. set the 109 already-proven rows to their audited terminal status;
2. set the 23 rows only after their task above is green;
3. regenerate the exact source mapping and hand-classify only genuinely new
   matches;
4. update release-readiness, deferred entries, changelog, and this plan's status;
5. run the full finish-remediation release gate and independent reviews.

Rows closed: `V06` and the final evidence rows.

## Final gate

Run sequentially from a clean worktree:

```bash
cargo nextest run --workspace
jake examples
jake smoke-bytecode
jake lint
jake docs-check
cargo check --target wasm32-unknown-unknown -p sema-wasm
scripts/check-unified-runtime-legacy.sh --check
scripts/check-unified-runtime-inventory.sh --check
scripts/test-packaged-sema-web.sh
```

Build the final playground WASM before the focused Playwright job. Completion
requires zero Critical or Important findings from independent runtime-core,
integration, WASM/browser, and evidence reviews.
