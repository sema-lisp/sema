# Unified Runtime Finish Remediation

**Status:** In progress 2026-07-18

## Goal

Close the whole-branch review findings before the unified runtime branch lands.
Each fix must have a regression that fails on the exact ownership, cancellation,
or verification boundary. Public behavior remains unchanged except where the
current implementation leaks, blocks, or violates an already documented
contract.

## Global constraints

- Preserve CORE-2 GC invariant I2 and keep executor futures `Send + 'static`.
- Runtime, root, and task identity must be explicit wherever state can outlive a
  single drive turn or cross an interpreter boundary.
- Cancellation must use one canonical origin-root cascade and run teardown once.
- Host resources must remain owned until cancellation or normal shutdown invokes
  their abort/close path.
- No timing-only concurrency oracle where a structural signal is available.
- Preserve the packaged-build invariant and keep browser acceptance in CI.
- Every Critical or Important review finding is fixed and independently
  re-reviewed before the next dependent task.

## Task 1: Runtime-quantum RAII

Make `RuntimeQuantumGuard` restore the displaced TLS value, including nested
drives and unwind. Remove manual set/reset windows around native dispatch.

Regression tests:

- nested runtime contexts restore the outer `in_runtime_quantum()` state;
- a panicking native cannot leave the flag set after `catch_unwind`.

## Task 2: Canonical cancellation and deadlock teardown

Route `RootHandle::cancel` through the same origin-root cascade as
`Runtime::cancel_root`. A matching cooperative debug stop must have its barrier
cleared and settle from ordinary root or command-handle cancellation. Deadlock
settlement must deregister protocol waits, observers, timers, and other owned
wait state before removing the main task.

Regression tests:

- handle cancellation reaps a fire-and-forget grandchild;
- command `cancel_root` and `cancel_all` settle a debug-paused root without a
  debug-specific resume;
- a persistent interpreter can deadlock on a promise, later wake its dependency,
  and continue without `protocol wake task disappeared`.

## Task 3: Runtime-scoped task identity

Replace bare task-number publication to thread-global task-owned registries with
a runtime-scoped identity. Carry the identity through current-task TLS and the
task-reaped callback. Update LLM agent/stream ownership keys and all consumers.

Regression test: two interpreters with colliding local task numbers retain their
own FakeProvider agent/stream state when one interpreter cancels its task.

## Task 4: MCP structural waits

Convert runtime calls for `mcp/connect`, `mcp/tools`, `mcp/tools->sema`, and
`mcp/close` from `io_block_on` simple natives to cancellable External waits.
Keep synchronous top-level behavior unchanged.

Regression tests use a delayed fake MCP server to prove sibling progress,
prompt cancellation, and result/error parity for every converted operation.

## Task 5: Resource-gate terminal lifecycle

Give every gated resource an explicit terminal close/tombstone path that emits
`CloseResourceGate`. Cover file streams, process/PTY, serial, SQLite, KV, MCP,
and every other mapping created through the resource-gate registry. Closing or
dropping a handle must return the runtime gate count to baseline.

## Task 6: `http/serve` host and request ownership

Retain the `axum::serve` abort hook as server-owned host state and invoke it on
root cancellation/drop so the port can be rebound. Tie each request future to
its spawned runtime task; dropping the client request cancels only that handler.
Preserve WebSocket bridge cleanup and server-root cascade behavior.

Replace the SRV-1 timing/race harness with structural entry signals and
failure-safe subprocess RAII. Tests must prove listener rebinding, per-request
disconnect cancellation without cancelling the server, and child cleanup on a
deliberate timeout.

## Task 7: Per-interpreter WASM Promise driver

Move Promise roots, scheduled-turn state, and interpreter selection out of
thread-global singleton ownership. Concurrent `SemaInterpreter` instances must
drive and cancel only their own roots even when local numeric IDs collide.

Browser regressions create two interpreters, settle concurrent suspending roots,
cancel one root, and prove the other completes normally.

## Task 8: Verification and evidence closure

- Make the runtime inventory checker reject nonterminal mapped rows; reconcile
  every ledger row to `MIGRATED`, `REMOVED`, or `SYNCHRONOUS-PROOF`.
- Add required CI coverage for the focused playground unified-runtime and stable
  debug-HTTP Playwright specs.
- Fix debugger gutter selectors against the installed UI contract and remove the
  false-red deferral.
- Make the earliest-timer unit test deterministic and reject a zero busy rearm.
- Reconcile release-readiness/deferred documents to HEAD or label point-in-time
  reports as superseded.
- Remove stale live-code comments naming deleted `AwaitIo`, yield-signal, and
  legacy-scheduler paths; sanitize transcript whitespace.

## Final gates

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

Run the focused required Playwright job on the final generated playground assets.
Then perform independent runtime-core, integration, and verification re-reviews
over the remediation range. The finish status changes to complete only when all
gates are green and no Critical or Important findings remain.
