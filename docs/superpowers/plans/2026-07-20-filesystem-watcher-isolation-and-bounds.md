# Filesystem Watcher Isolation and Bounds Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make filesystem watcher handles, queues, sandbox authority, and teardown evaluator-owned and bounded without blocking the evaluator on platform watcher registration.

**Architecture:** Replace the ambient thread-local map with one host-only `Rc<WatchRegistry>` captured by the three builtins installed into an evaluator environment. Register a weak context teardown hook for each live epoch, reserve worker capacity until the worker actually exits, and deliver events through a bounded nonblocking queue with an explicit overflow record.

**Tech Stack:** Rust 2021, `notify`, `Rc`/`Weak`, `std::sync::mpsc::sync_channel`, Sema `EvalContext` teardown hooks.

## Global Constraints

- Preserve the public names and integer-handle API.
- A capability or path denial must occur before filesystem inspection, capacity reservation, or thread creation.
- Native closures may capture the host-only registry and sandbox, but never `Value`, `Env`, or an evaluator context.
- Interpreter teardown must remain bounded and must not join a possibly blocked platform registration.
- The active-thread lease, not public handle count, enforces the per-evaluator limit.
- Platform registration readiness and cancellation remain explicitly nonterminal.
- Preserve unrelated worktree changes and stage only owned files.

---

### Task 1: Add isolation and sandbox RED tests

**Files:**
- Modify: `crates/sema/tests/integration_test.rs`

- [ ] **Step 1: Prove handle isolation**

Create two independent `Interpreter`s and a temporary directory. Register the first watch in interpreter A and assert its first handle is `1`. Before registering any watcher in B, call `(fs/watch-events 1)` and assert `SemaError` reports no such watcher. Call `(fs/unwatch 1)` in B, then verify A can still drain its handle and unwatch it.

- [ ] **Step 2: Prove sandbox path enforcement**

Create two real temporary directories. Build an interpreter whose allowed paths contain only the first, then call `fs/watch` on the second. Assert the structural error is `SemaError::PathDenied`.

- [ ] **Step 3: Prove interpreter teardown with a retained environment**

Register a watcher, retain `interp.global_env`, then drop the interpreter. Retrieve the retained `fs/watch-events` native and invoke it with a fresh `EvalContext`; assert the old handle is gone. This proves the context teardown hook clears the registry rather than relying on environment collection.

- [ ] **Step 4: Run the focused tests and verify RED**

```bash
cargo nextest run -p sema-lang --test integration_test 'fs_watch_interpreter|fs_watch_respects|fs_watch_teardown'
```

Expected: isolation, path enforcement, and retained-environment teardown fail against the ambient registry.

### Task 2: Implement the evaluator-owned registry

**Files:**
- Modify: `crates/sema-stdlib/src/fs_watch.rs`

- [ ] **Step 1: Replace ambient state**

Add a `WatchRegistry` containing a `RefCell<HashMap<i64, Watch>>`, checked next-handle allocation, an `Arc<WorkerCapacity>`, and a `Cell<bool>` teardown-hook marker. Construct one registry in `register` and capture `Rc` clones in all three native functions. Delete `WATCHERS` and `NEXT_ID`.

- [ ] **Step 2: Make `fs/watch` context-aware and fully checked**

Install `fs/watch` with `NativeFn::with_ctx`. Parse its arguments, then call `sandbox.check(Caps::FS_READ, "fs/watch")` and `sandbox.check_path(path, "fs/watch")` before `Path::exists`, capacity reservation, or thread spawn.

On the first live epoch, register a teardown hook that captures `Weak<WatchRegistry>` and calls `stop_all`. `stop_all` clears entries and resets the marker without joining workers.

- [ ] **Step 3: Bound worker lifetime**

Reserve one of 64 worker slots before spawning. Move an RAII lease into the worker so capacity is returned only when that thread exits. Use `thread::Builder::spawn` and return `SemaError::Io` on failure; dropping the untransferred lease rolls the reservation back.

The worker constructs the `notify` watcher, attempts registration, then waits on the stop channel. It must also observe a stop already issued while construction or registration was in progress.

- [ ] **Step 4: Preserve local handle behavior**

Allocate and insert the handle only in the captured registry. `fs/watch-events` rejects missing local handles. `fs/unwatch` removes only a local entry and remains idempotent for an unknown handle.

### Task 3: Bound event delivery and test the internal invariants

**Files:**
- Modify: `crates/sema-stdlib/src/fs_watch.rs`

- [ ] **Step 1: Add a bounded nonblocking event sink**

Replace `channel` with `sync_channel(EVENT_QUEUE_CAPACITY)`. The `notify` callback uses `try_send`; on `Full`, increment a saturating `AtomicUsize`, and on `Disconnected`, return. Do not block a platform callback.

- [ ] **Step 2: Expose overflow explicitly**

After draining retained events, swap the dropped counter to zero. If it was nonzero, append:

```sema
{:kind :overflow :paths () :dropped N}
```

The batch stays bounded by 1,025 result entries and the next drain does not repeat an already-reported count.

- [ ] **Step 3: Add module-level invariant tests**

Test capacity reservation/release without spawning 64 platform watchers. Test that removing a handle does not release a still-live worker lease. Feed more than 1,024 synthetic events through the sink, assert the queue remains bounded, assert the exact overflow count, and assert the counter resets after drain.

- [ ] **Step 4: Run focused tests to verify GREEN**

```bash
cargo nextest run -p sema-stdlib fs_watch
cargo nextest run -p sema-lang --test integration_test 'fs_watch_interpreter|fs_watch_respects|fs_watch_teardown'
```

Expected: all new and existing watcher tests pass.

### Task 4: Verify, document the partial boundary, and hand off for review

**Files:**
- Modify: `docs/plans/2026-07-19-unified-runtime-terminal-inventory.md`
- Verify: `crates/sema-stdlib/src/fs_watch.rs`
- Verify: `crates/sema/tests/integration_test.rs`

- [ ] **Step 1: Record the exact row split**

Mark watcher handle isolation, sandbox checking, queue/thread caps, and interpreter teardown proven. Keep platform watcher registration readiness and cancellation nonterminal because a thread blocked inside `notify` cannot observe teardown until the backend returns.

- [ ] **Step 2: Run focused quality gates**

```bash
cargo fmt --check
cargo clippy -p sema-stdlib --all-targets -- -D warnings
cargo nextest run -p sema-lang --test integration_test 'fs_watch'
cargo check --target wasm32-unknown-unknown -p sema-wasm
git diff --check -- crates/sema-stdlib/src/fs_watch.rs crates/sema/tests/integration_test.rs docs/plans/2026-07-19-unified-runtime-terminal-inventory.md
```

Expected: every command exits 0, apart from already-documented unrelated wasm warnings if the repository still emits them.

- [ ] **Step 3: Commit only owned files**

Temporarily unstage the eight protected frontend files, stage only the watcher implementation/tests/inventory, commit with `fix(stdlib): isolate filesystem watcher ownership`, then restore the exact protected staged set. Never stage `crates/sema-vm/src/runtime/state.rs`.

- [ ] **Step 4: Request an independent adversarial review**

The review must verify evaluator isolation, sandbox check order, bounded queue and worker leases, teardown with a retained environment, CORE-2 closure safety, and honest nonterminal classification of platform registration.
