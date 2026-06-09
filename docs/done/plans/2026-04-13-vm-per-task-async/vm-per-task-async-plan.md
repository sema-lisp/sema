# VM-per-Task Async Concurrency

> **Initiated**: 2026-04-13 · **Last touched**: 2026-04-13 · **Status**: Draft
>
> **Prerequisite**: VM is now the default backend (commit `72e9ae9` on main). CLI uses `--tw` to opt into tree-walker. Notebook and playground also default to VM.

## Decision Question

How should Sema implement cooperative async concurrency without the side-effect corruption issues of the replay model?

---

## Scope Assessment

| Dimension | Rating | Notes |
|---|---|---|
| Size | L | 7 crates touched, new scheduler subsystem |
| Type | Language feature + Refactor | Replace replay async with VM-per-Task model |
| Surfaces | Core + Stdlib + VM + Eval + Binary | Value types stay, eval gets gutted for async, VM gets scheduler |
| Risk | High | Touches SemaError enum, VM instantiation, stdlib async_ops, special forms in both backends |

---

## Goal

Replace PR #29's replay-based async scheduler with a VM-per-Task model where each spawned task gets its own VM instance sharing globals and the compiled function table. This eliminates the fundamental flaw of the replay model (re-execution of side effects) while keeping the same user-facing API.

Secondary goal: make async VM-only and remove the dual-eval requirement for async features. The VM is already the default backend (as of `72e9ae9`), so async will "just work" for all users without any flag.

---

## Current State

PR #29 (branch `feature/async-concurrency`, open) adds:
- `AsyncPromise` (tag 28), `Channel` (tag 29) value types in sema-core
- `SemaError::Yield(YieldReason)` variant with `unsafe impl Send/Sync`
- Replay-based scheduler in `sema-stdlib/src/async_ops.rs` with thread-local state (ReplayState, REPLAY, IN_ASYNC_TASK, SUPPRESS_OUTPUT, PENDING_REPLAY)
- `async`/`await` special forms in both tree-walker (`special_forms.rs`) and VM (`lower.rs`)
- Yield propagation guards in `eval.rs` (3 places) and `vm.rs` (1 place in exception handler)
- 62 dual-eval async tests

**Why it must change**: The replay model re-runs task bodies from scratch on each resume. Side effects (println, file/write, set!, random) execute on every replay. Research confirmed this cannot be fixed by expanding the replay log — return value caching doesn't prevent duplicate external effects.

**Existing infrastructure we can build on**:
- `VM::new_with_rc_functions()` (line 180) already creates a VM with shared `Rc<Vec<Rc<Function>>>` — exactly what VM-per-Task needs
- `VmExecResult` enum already has `Finished`, `Stopped`, `Yielded` variants (used by DAP debugger)
- `run_cooperative` / `start_cooperative` demonstrate the VM can suspend and resume mid-execution
- `AsyncPromise`, `Channel`, `PromiseState` value types are correct and stay as-is
- The user-facing API (async/spawn, async/await, channel/new, etc.) stays identical

**Related decisions**: Decision #4 (Trampoline TCO), Decision #50 (Same-VM closure execution), Decision #51 (True TCO for VM closures)

---

## Crate Map

| Crate | File | Action | Purpose |
|---|---|---|---|
| `sema-core` | `src/error.rs` | Modify | Remove `SemaError::Yield` variant, remove `YieldReason` enum, remove `unsafe impl Send/Sync` |
| `sema-core` | `src/lib.rs` | Modify | Remove `YieldReason` from exports |
| `sema-core` | `src/async_signal.rs` | Create | YieldReason enum, YIELD_SIGNAL/RESUME_VALUE/IN_ASYNC_CONTEXT thread-locals, set/take functions |
| `sema-core` | `src/value.rs` | No change | AsyncPromise, Channel types stay as-is |
| `sema-eval` | `src/eval.rs` | Modify | Remove 3 Yield propagation guards |
| `sema-eval` | `src/special_forms.rs` | Modify | `async`/`await` return error "async requires the VM backend (do not use --tw)" |
| `sema-vm` | `src/vm.rs` | Modify | Add `VM::new_for_task()` constructor, add `AsyncYield` to `VmExecResult`, modify native call dispatch to handle yield signal |
| `sema-vm` | `src/debug.rs` | Modify | Add `AsyncYield(YieldReason)` variant to `VmExecResult` |
| `sema-vm` | `src/lower.rs` | No change | `async`/`await` lowering stays (already desugars to async/spawn and async/await calls) |
| `sema-vm` | `src/scheduler.rs` | Create | Task scheduler: manages VM instances, round-robin execution, wake/block logic |
| `sema-stdlib` | `src/async_ops.rs` | Rewrite | Remove replay infrastructure entirely. Channel/async ops signal yield via thread-local VM context, not SemaError |
| `sema-stdlib` | `src/lib.rs` | No change | async_ops::register still called |
| `sema` | `tests/dual_eval_test.rs` | Modify | Remove async dual-eval tests |
| `sema` | `tests/vm_async_test.rs` | Create | VM-only async tests (migrated from dual-eval) |
| `sema` | `tests/integration_test.rs` | Possibly modify | Remove any async tree-walker tests if present |
| Root | `CLAUDE.md` | Modify | Update testing guidance: async tests are VM-only, note tree-walker deprecation path |

---

## Proposed Changes

### 1. Remove SemaError::Yield and all replay infrastructure

**What**: Delete the `Yield(YieldReason)` variant from `SemaError`, the `YieldReason` enum (and its `unsafe Send/Sync`), all Yield propagation guards in eval.rs (3 sites) and the try/catch bypass in special_forms.rs and vm.rs.

**Why**: Yield-as-error was a hack to propagate suspension through the tree-walker. The VM-per-Task model doesn't need it — suspension is handled by the VM returning `VmExecResult::AsyncYield` from its run loop.

**Approach**: Delete code, fix compiler errors. This is a pure removal — any compilation failure points to remaining Yield references that need updating.

### 2. Move YieldReason to sema-core::async_signal

**What**: `YieldReason` moves from `sema-core::error` to a new `sema-core::async_signal` module. It no longer needs `Send/Sync` since it never enters `SemaError`. It lives in sema-core (not sema-vm) because sema-stdlib needs to reference it and can only import from sema-core.

**Why**: YieldReason uses Rc pointers to AsyncPromise and Channel (both sema-core types). The yield signal thread-locals also go in sema-core, following the same pattern as the existing `call_callback`/`eval_callback` thread-locals in `context.rs`.

**Fields**: Same as PR #29 — `AwaitPromise(Rc<AsyncPromise>)`, `ChannelRecv(Rc<Channel>)`, `ChannelSend(Rc<Channel>, Value)`, `Sleep(u64)`.

### 3. VM yield mechanism via thread-local signal

**What**: When a native function (e.g., `channel/recv`) needs to yield, it sets a thread-local `YIELD_SIGNAL: Cell<Option<YieldReason>>` and returns `Ok(Value::nil())` (placeholder). The VM's native call dispatch checks this signal after every native call; if set, it saves the current pc and returns `VmExecResult::AsyncYield(reason)`.

**Why**: NativeFn's signature is `fn(&EvalContext, &[Value]) -> Result<Value, SemaError>`. We cannot change this without touching every native function. A thread-local signal lets async-aware natives communicate with the VM without changing the NativeFn interface. The scheduler stores the resume value and the VM replays just the single native call on resume.

**Key detail**: On resume, the scheduler sets a thread-local `RESUME_VALUE: Cell<Option<Value>>`. The native function checks this first — if set, returns it immediately instead of performing the operation. This is a **single-operation replay**, not full-body replay. Only the one native call that yielded replays, and it's a single cell check.

**Alternative considered**: Adding a `Yield` variant to `Result<Value, SemaError>`. Rejected because it requires `SemaError` to carry Rc pointers (the unsafe Send/Sync problem we're removing) and pollutes every error handler in the codebase.

### 3b. Spawn callback (stdlib → scheduler bridge)

**What**: `async/spawn` in sema-stdlib needs to create a task in the scheduler (sema-vm). Since sema-stdlib can't depend on sema-vm, we add a `SpawnCallbackFn` thread-local in sema-core, registered by the scheduler at startup. Same pattern as `set_eval_callback` / `set_call_callback`.

**Signature**: `type SpawnCallbackFn = fn(&EvalContext, Value) -> Result<Value, SemaError>` — takes the thunk, returns the promise. The scheduler's implementation compiles the thunk, creates a VM, and registers the task.

### 4. VM-per-Task scheduler

**What**: New `sema-vm/src/scheduler.rs` module. Thread-local `Scheduler` that manages a list of `Task` structs, each owning a `VM` instance.

**Core types**:
```
struct Task {
    id: u64,
    vm: VM,
    closure: Rc<Closure>,       // The task's entry point
    promise: Rc<AsyncPromise>,  // Result destination
    state: TaskState,           // Ready / Blocked(YieldReason) / Done / Failed
    started: bool,              // Whether execute has been called
}

enum TaskState {
    Ready,
    Blocked(YieldReason),
    Done,
    Failed,
}
```

**Scheduler operations**:
- `spawn(closure, globals, functions, ctx) -> Rc<AsyncPromise>`: Create a new Task with its own VM instance via `VM::new_for_task(globals, functions)`. Returns the promise.
- `run_until(ctx, target_promise) -> Result<(), SemaError>`: Event loop — round-robin ready tasks, wake blocked tasks, detect deadlock. Exits when target resolves or all tasks finish.
- `run_all(ctx)`: Run all tasks to completion.
- `run_one_step(ctx, task_idx)`: Execute a single task. Calls `vm.execute(closure, ctx)` for new tasks or `vm.run_cooperative(ctx, debug)` for resumed tasks. Checks yield signal after return.

**Wake logic** (same as PR #29, proven correct):
- `AwaitPromise(p)`: wake when `p.state != Pending`
- `ChannelRecv(ch)`: wake when `ch.buffer` non-empty or `ch.closed`
- `ChannelSend(ch, val)`: wake when `ch.buffer.len() < ch.capacity`
- `Sleep(ms)`: wake immediately (sleep duration not enforced in cooperative model)

**VM sharing**: Each task's VM shares `Rc<Env>` globals and `Rc<Vec<Rc<Function>>>` function table with the parent VM. `VM::new_for_task()` uses the existing `new_with_rc_functions()` pattern. Each VM gets its own stack, frames, and inline cache.

### 5. Rewrite async_ops.rs stdlib registration

**What**: Strip all replay infrastructure. Keep the same function names and signatures. Channel ops and async/await use the yield signal mechanism.

**Key changes per function**:
- `async/spawn`: Call `Scheduler::spawn()` instead of pushing to thread-local task list
- `async/await`: If promise resolved, return value. If pending and in async context, set `YIELD_SIGNAL` to `AwaitPromise`. If at top level, call `Scheduler::run_until()`
- `channel/recv`: If buffer non-empty, pop and return. If empty and in async context, set `YIELD_SIGNAL` to `ChannelRecv`. If empty outside async, error.
- `channel/send`: If buffer has space, push and return. If full and in async context, set `YIELD_SIGNAL` to `ChannelSend`. If full outside async, error.
- `async/all`, `async/race`, `async/run`: Delegate to scheduler
- Predicates (`async/promise?`, `channel?`, etc.): No changes needed
- `channel/new`, `channel/close`, `channel/try-recv`, `channel/count`, `channel/empty?`, `channel/full?`, `channel/closed?`: No yield behavior, no changes needed

### 6. Tree-walker async deprecation

**What**: `eval_async` and `eval_await` in `special_forms.rs` return `SemaError::eval("async/await requires the VM backend (do not use --tw)")`.

**Why**: The tree-walker cannot support real coroutines without continuations. Since VM is now the default, this only affects users who explicitly opt into `--tw`. A clear error guides them back.

### 7. Update CLAUDE.md and test infrastructure

**What**: 
- Remove "Both must produce identical results" / "Any new language feature must be tested through both backends" language for async features
- Add note: "Async features (async/await, channels) are VM-only. Tests go in `vm_async_test.rs`."
- Update "Adding New Functionality" section to note async features are VM-only
- Move 62 async tests from `dual_eval_test.rs` to new `vm_async_test.rs` (VM-only)

---

## Implementation Phases

### Phase 1: Clean up — Remove replay and Yield infrastructure

> **Status**: Not started

1. **Remove `SemaError::Yield` and `YieldReason` from sema-core**
   - Crate: `sema-core`
   - Files: `src/error.rs`, `src/lib.rs`
   - Do: Delete `YieldReason` enum, `unsafe impl Send/Sync`, `Yield(YieldReason)` variant from SemaError. Remove from exports.
   - Verify: `cargo check -p sema-core`

2. **Remove Yield propagation guards from tree-walker**
   - Crate: `sema-eval`
   - Files: `src/eval.rs`, `src/special_forms.rs`
   - Do: Remove 3 `if matches!(&e, SemaError::Yield(_))` guards in eval.rs. Remove Yield bypass in `eval_try`. Remove `unreachable!("Yield should never reach error_to_value")`. Replace `eval_async`/`eval_await` with error stubs.
   - Verify: `cargo check -p sema-eval`

3. **Remove Yield handling from VM**
   - Crate: `sema-vm`
   - Files: `src/vm.rs`
   - Do: Remove Yield check in `handle_exception`. Remove Yield arm in `error_to_value`.
   - Verify: `cargo check -p sema-vm`

4. **Gut replay infrastructure from async_ops.rs**
   - Crate: `sema-stdlib`
   - Files: `src/async_ops.rs`
   - Do: Remove `ReplayState`, `REPLAY`, `IN_ASYNC_TASK`, `SUPPRESS_OUTPUT`, `PENDING_REPLAY` thread-locals. Remove `replay_check`, `replay_record`, `is_output_suppressed`, `is_in_async_task`. Remove `Scheduler` struct and `Task`/`TaskState` (will be rebuilt in Phase 2). Keep function registrations as stubs returning `SemaError::eval("async: not yet reimplemented")`.
   - Verify: `cargo test -p sema-stdlib`

5. **Remove async dual-eval tests**
   - Crate: `sema`
   - Files: `tests/dual_eval_test.rs`
   - Do: Remove the "Async concurrency" dual_eval_tests! and dual_eval_error_tests! blocks (last ~195 lines).
   - Verify: `cargo test -p sema --test dual_eval_test`

**Phase exit criteria**: `make test` passes with all async functionality temporarily stubbed out. No `YieldReason` or `SemaError::Yield` anywhere in the codebase. No `unsafe impl Send/Sync` for async types.

### Phase 2: VM yield mechanism and scheduler

> **Status**: Not started

1. **Add yield signal infrastructure to sema-core**
   - Crate: `sema-core`
   - Files: `src/async_signal.rs` (new), `src/lib.rs`
   - Do: Create `YieldReason` enum (AwaitPromise, ChannelRecv, ChannelSend, Sleep). Add `YIELD_SIGNAL: RefCell<Option<YieldReason>>`, `RESUME_VALUE: Cell<Option<Value>>`, `IN_ASYNC_CONTEXT: Cell<bool>` thread-locals. Add `set_yield_signal()`, `take_yield_signal()`, `set_resume_value()`, `take_resume_value()`, `in_async_context()`, `set_async_context()` public functions. Export from lib.rs.
   - Verify: `cargo check -p sema-core`

2. **Add `AsyncYield` to `VmExecResult`**
   - Crate: `sema-vm`
   - Files: `src/debug.rs`
   - Do: Add `AsyncYield(YieldReason)` variant. Import `YieldReason` from `sema_core::async_signal`.
   - Verify: `cargo check -p sema-vm`

3. **Add yield signal check to VM native call dispatch**
   - Crate: `sema-vm`
   - Files: `src/vm.rs`
   - Do: After native fn call at line ~789, check `take_yield_signal()`. If Some, save pc (already saved at line 767), return `VmExecResult::AsyncYield(reason)`. Need to plumb this through `run_inner` which currently returns `VmExecResult`. Also handle for `CALL_GLOBAL` path where native is called via `call_callback`.
   - Verify: `cargo check -p sema-vm`

4. **Add `VM::new_for_task()` constructor**
   - Crate: `sema-vm`
   - Files: `src/vm.rs`
   - Do: Public constructor that takes `Rc<Env>`, `Rc<Vec<Rc<Function>>>`, builds native_fns table. Essentially a public version of the existing `new_with_rc_functions` that also resolves the native table.
   - Verify: Unit test: create two VMs sharing globals, execute in each

5. **Implement `Scheduler`**
   - Crate: `sema-vm`
   - Files: `src/scheduler.rs`
   - Do: `Task` struct (id, vm, closure, promise, state, started). `Scheduler` with `spawn()`, `run_until()`, `run_all()`, `wake_blocked_tasks()`, `run_task()`. Thread-local `SCHEDULER`. Round-robin execution. Deadlock detection (all blocked, none ready). Max-ticks guard (1M).
   - Verify: Unit test: spawn two tasks, one sends to channel, other receives

6. **Handle resume after yield**
   - Crate: `sema-vm`
   - Files: `src/scheduler.rs`, `src/vm.rs`
   - Do: On resume, scheduler sets `RESUME_VALUE` with the wake value, then calls `vm.run_cooperative()` (pc is already saved from the yield point, pointing past the CALL_NATIVE that triggered yield). The native fn checks `take_resume_value()` first. Need `VM::resume_cooperative()` or use existing `run_cooperative` with debug=None.
   - Verify: Unit test: task yields on channel recv, resume returns the value

**Phase exit criteria**: Scheduler can spawn tasks, execute them on separate VMs, handle yield/resume for channel ops and await. `cargo test -p sema-vm` green.

### Phase 3: Wire up stdlib and end-to-end tests

> **Status**: Not started

1. **Rewrite async_ops.rs with yield signal mechanism**
   - Crate: `sema-stdlib`
   - Files: `src/async_ops.rs`
   - Do: Replace stub implementations with real ones. Channel ops use `sema_core::{set_yield_signal, take_resume_value, in_async_context}`. `async/spawn` uses a new thread-local spawn callback (registered by sema-vm scheduler at startup, same pattern as eval_callback). Channel ops that need to yield call `set_yield_signal()` and return `Ok(Value::nil())`.
   - Verify: `cargo check -p sema-stdlib`
   - **Dependency note**: sema-stdlib imports only from sema-core. All yield signals and spawn callback are in sema-core. The scheduler in sema-vm registers the spawn callback and reads yield signals — same layering as eval_callback.

2. **Create VM-only async test file**
   - Crate: `sema`
   - Files: `tests/vm_async_test.rs` (new)
   - Do: Migrate all 62 async tests from the old dual-eval block. Run through VM backend only. Add new tests for side-effect correctness (println, set!, file operations don't repeat on resume).
   - Verify: `cargo test -p sema --test vm_async_test`

3. **End-to-end integration**
   - Crate: `sema`
   - Files: `src/main.rs`
   - Do: Ensure default (VM) runs async correctly. Ensure `--tw` gives clear error on async. Wire up scheduler initialization in the VM execution path.
   - Verify: `cargo run -- -e "(let ((p (async (+ 1 2)))) (await p))"` (no flag needed — VM is default)

**Phase exit criteria**: All 62 async tests pass (VM-only). New side-effect correctness tests pass. `make test` fully green. `--tw` gives clear error on async forms.

### Phase 4: Documentation and cleanup

> **Status**: Not started

1. **Update CLAUDE.md**
   - File: `CLAUDE.md`
   - Do: Update Testing section — async tests are VM-only in `vm_async_test.rs`. Update "Adding New Functionality" — async features are VM-only. Add note about tree-walker deprecation path for async. Keep dual-eval requirement for all non-async features.
   - Verify: Read the file, check accuracy

2. **Update DECISIONS.md**
   - File: `docs/adr.md`
   - Do: Add Decision #53 (VM-per-Task async) and Decision #54 (async is VM-only)
   - Verify: Read the file

3. **Clean up PR #29 branch**
   - Do: Either close PR #29 and open a new PR, or force-push the new implementation onto the same branch
   - Verify: PR review

**Phase exit criteria**: All documentation updated. `make all` passes. PR ready for review.

---

## Supporting Documents

| Document | Purpose |
|---|---|
| [Research notes](./vm-per-task-async-research.md) | Replay model analysis, approach comparison, CPS feasibility |

---

## Questions to Resolve

- [x] **[High]** Where do yield signal thread-locals live? — **Answer: sema-core**, following the callback pattern. See Phase 3, item 1.
- [ ] **[High]** How does `VM::resume_cooperative()` work? The existing `run_cooperative` requires a `DebugState`. Need a variant that works without debug state, or make debug state optional. Check if we can use `run_inner(ctx, None)` directly since the frame/pc is already saved.
- [ ] **[Low]** Should `async/sleep` actually enforce timing? Current model (both replay and VM-per-task) treats sleep as immediate wake. Real timing would need a wall-clock check in wake_blocked_tasks.
- [ ] **[Low]** Should we add `async/timeout` in this iteration? PR #29 lists it as a known gap. Defer to follow-up.

---

## Key Assumptions

| # | Assumption | Drives which decision | How to validate | Status |
|---|---|---|---|---|
| A1 | `VM::new_with_rc_functions()` creates a fully functional VM that can share globals | VM-per-Task architecture | Create two VMs from same globals, execute independently | Untested |
| A2 | `VmExecResult::Yielded` + `run_cooperative` can resume a VM mid-native-call | Resume mechanism | The VM saves pc before native call (line 767). After resume, pc points to instruction after the CALL_NATIVE. Need to verify the native call result is properly placed on stack. | Untested |
| A3 | Thread-local yield signal is safe because Sema is single-threaded | Yield mechanism design | Already true for all thread-locals in sema-core (callbacks, sandbox ctx) | Confirmed |
| A4 | sema-stdlib can import from sema-core's new async_signal module | Dependency flow | sema-stdlib already depends on sema-core | Confirmed |
| A5 | Existing `run_cooperative` can work without DebugState or can be adapted | Resume mechanism | Read vm.rs — `run_inner` takes `Option<&mut DebugState>`, debug checks are gated on `if let Some(debug)` | Untested |
| A6 | Task VM instances don't need the native_fns table if using CALL_GLOBAL for async ops | VM instantiation | Check if async ops (channel/recv etc.) are compiled as CALL_GLOBAL or CALL_NATIVE. Since the task body is compiled in the parent context, it may use CALL_NATIVE. If so, new_for_task must also resolve native table. | Untested |
| A7 | Spawn callback pattern works for async/spawn (same as eval_callback) | Stdlib-to-scheduler bridge | sema-core already has set_call_callback/set_eval_callback thread-locals used by sema-stdlib. Spawn callback follows same pattern. | Confirmed (pattern exists) |

---

## Design Decisions

### Decision #53: VM-per-Task cooperative async

Each `async/spawn` creates a new VM instance with its own stack and frames, sharing `Rc<Env>` globals and `Rc<Vec<Rc<Function>>>` with the parent. A round-robin scheduler manages tasks. Yield is signaled via thread-local, not error variants. Replaces the replay model which corrupted side effects.

### Decision #54: Async features are VM-only

`async`, `await`, channels, and the task scheduler require the VM backend. The tree-walker returns a clear error. This acknowledges the tree-walker's deprecation path and avoids maintaining two async implementations.

---

## Technical Debt

- `async/sleep` doesn't enforce real timing — yields and immediately wakes. Acceptable for cooperative scheduling but should be documented.
- Task VMs each allocate their own inline cache. For many small tasks this wastes memory. Could share or pool caches later.
- No task cancellation or timeout mechanism. Deferred to follow-up.
- `run_cooperative` may need refactoring to cleanly support both debug and async use cases. Currently the non-debug path uses `run()` which doesn't return `VmExecResult`.

---

## Checklist

### Analysis
- [x] **Existing code reviewed** — VM struct, run_inner, VmExecResult, new_with_rc_functions, async_ops.rs, special_forms.rs
- [x] **Edge cases identified** — Deadlock detection, max ticks, yield inside try/catch, nested async/spawn
- [x] **Dependency flow verified** — Yield signals in sema-core (like callbacks), scheduler in sema-vm, stdlib uses sema-core signals
- [x] **Performance implications considered** — VM-per-task has memory overhead per task (stack + cache); negligible for realistic task counts

### Bytecode
- [ ] **Opcode design finalized** — No new opcodes needed. async/await lowering already desugars to function calls (async/spawn, async/await). Yield is signaled out-of-band via thread-local.
- [ ] **Bytecode format spec updated** — No changes needed
- [ ] **Serialization updated** — No changes needed

### Compatibility
- [x] **Breaking changes documented** — async/await only works with VM backend (default). `--tw` users get clear error.
- [x] **Migration path described** — No migration needed; VM is already the default. Only `--tw` users are affected.
- [x] **Naming convention followed** — All existing names preserved (async/spawn, channel/recv, etc.)

### Testing
- [ ] **VM-only async tests planned** — Migrate 62 tests to vm_async_test.rs
- [x] **Test scenarios identified** — All PR #29 scenarios plus: side-effect correctness (println count, set! value, file write count)
- [x] **Existing test coverage reviewed** — 62 dual-eval tests cover spawn/await, channels, producer/consumer, race, yield-through-try, error cases
- [x] **I/O or network tests separated** — Async tests go in vm_async_test.rs (VM-only)

### Adversarial Review
- [x] **Assumptions surfaced** — 6 assumptions listed with validation methods
- [x] **Assumptions stress-tested** — A2 (resume mid-native) and A6 (native table for task VMs) are highest risk
- [x] **Crate map complete** — 15 files across 5 crates

### Maintainability
- [x] **Complexity assessed** — Scheduler is the main new code (~300-400 lines). Each function is small and focused.
- [x] **Coupling reviewed** — sema-core holds signals (minimal), sema-vm holds scheduler, sema-stdlib calls through sema-core signals. Clean layering.
- [x] **Pattern consistency** — Thread-local signal pattern matches existing callback architecture (Decision #6)
- [x] **Callback architecture respected** — Stdlib still calls eval via call_callback. Yield signal is a parallel out-of-band channel.

### WASM
- [ ] **Playground impact assessed** — Async/channels not used in sema-wasm currently. No WASM changes needed.
- [ ] **Browser limitations identified** — WASM is single-threaded, cooperative scheduling works fine. No timer for sleep.
- [ ] **Graceful degradation planned** — Async should work in WASM since it's cooperative. May need testing later.
