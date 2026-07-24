# WASM Debugger Admission Coordination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent legacy synchronous debugger roots and Promise-driven roots from being admitted concurrently on the same WASM interpreter without reducing concurrency across interpreters or within the Promise driver.

**Architecture:** Tag the thread-local legacy debug session with a weak interpreter owner and compare `Rc` identity at each Promise admission boundary. Expose the Promise driver's exact active-root predicate to reject legacy `debugStart` before parsing or expansion, and defensively cancel any already-submitted compiled root that reaches `adopt` after admission changed.

**Tech Stack:** Rust 2021, `Rc`/`Weak`, wasm-bindgen, Sema's persistent VM runtime, Playwright.

## Global Constraints

- Ordinary `evalPromise` roots and the Promise debugger must continue to coexist.
- A legacy debugger on interpreter A must not block Promise work on interpreter B.
- Legacy debugger methods on interpreter B must not inspect, replace, resume, stop, or mutate interpreter A's session.
- A dead legacy-session owner must be cancelled and evicted rather than treated as a permanently live foreign owner.
- Registered JavaScript callbacks may re-enter debugger and Promise APIs during macro expansion or runtime driving without a `RefCell` borrow panic or ownership gap.
- Legacy `debugStart` must reject before stopping a session, clearing output, parsing, macro expansion, compilation, or root submission.
- Source evaluation, compiled entry submission/adoption, and Promise debugger start must all enforce admission.
- Preserve unrelated worktree changes and stage only owned files.

---

### Task 1: Browser-first ownership regressions

**Files:**
- Modify: `playground/tests/unified-runtime.spec.ts`

**Interfaces:**
- Consumes: `SemaInterpreter.evalPromise`, `debugStart`, `debugContinue`, `debugStartPromise`, and independent interpreter instances.
- Produces: regression coverage for both mixed orderings, macro-expansion non-mutation, orphan-root non-execution, and interpreter isolation.

- [ ] **Step 1: Add the legacy-first rejection test**

Add a Playwright page test that pauses `debugStart('(+ 1 2)', [])`, calls `evalPromise('(context/set :legacy-promise-orphan 99)', onRoot)`, and calls `debugStartPromise('(defmacro promise-debug-admission-leak () 7)\n(+ 1 2)', [])`. Resume the legacy session, wait one browser turn, then inspect the marker through a fresh legacy debug run and probe the macro through `evalVM`.

Assert:

```ts
expect(result.entry).toMatchObject({ status: 'stopped' });
expect(result.promiseRoot).toBeNull();
expect(result.rejected.value).toBeNull();
expect(result.rejected.error).toContain('synchronous debugger');
expect(result.promiseDebug).toMatchObject({ status: 'error' });
expect(result.promiseDebug.error).toContain('synchronous debugger');
expect(result.inspected).toMatchObject({ status: 'finished', value: null });
expect(result.macroProbe.value).toBeNull();
expect(result.macroProbe.error.toLowerCase()).toContain('unbound variable');
```

- [ ] **Step 2: Add the Promise-first rejection test**

Submit this root and retain its synchronous root id:

```ts
const pending = interp.evalPromise(
  '(async/sleep 30)\n' +
    '(context/set :promise-admission-runs (+ (or (context/get :promise-admission-runs) 0) 1))\n' +
    '"done"',
  (root: number) => { promiseRoot = root; },
);
```

Immediately call legacy `debugStart` with a macro definition and a runtime marker. If the old implementation returns `stopped`, call `debugStop()` so the RED test terminates. Await the Promise and then probe the counter, marker, and macro.

Assert that the legacy result is an admission error, the Promise has a root and resolves to `"done"`, the counter is exactly `1`, the rejected debugger marker is nil, and the macro remains unbound.

- [ ] **Step 3: Add the cross-interpreter isolation test**

Pause a legacy debugger on interpreter A. On interpreter B, run:

```ts
interpB.evalPromise(
  '(async/sleep 20)\n(context/set :interpreter-b-runs 1)\n"B-ok"',
  (root: number) => { rootB = root; },
)
```

Assert B reports a root, resolves to `"B-ok"`, and records the marker while A remains active and then finishes normally when continued.

- [ ] **Step 4: Run the new tests and verify RED**

Before the RED run, add one cross-interpreter legacy-API test. Pause A, call B's active-state, locals, stack, continue, breakpoint, start, and stop APIs, and assert they neither observe nor mutate A. B's `debugStart` must reject before expanding a macro, A must still finish with its original breakpoint state, and B's `debugStop` must leave a second A session active.

Run:

```bash
cd playground
npx playwright test tests/unified-runtime.spec.ts --grep 'legacy debugger excludes|Promise root excludes|legacy debugger on interpreter A'
```

Expected: the same-interpreter tests fail because mixed ownership is currently admitted; the interpreter-isolation test passes.

### Task 2: Per-interpreter admission implementation

**Files:**
- Modify: `crates/sema-wasm/src/lib.rs`
- Modify: `crates/sema-wasm/src/driver.rs`

**Interfaces:**
- Consumes: `DebugSession`, `PromiseDriver`, `Interpreter::submit_str`, `Interpreter::submit_compile_result`, and Promise debugger root submission.
- Produces: `legacy_debug_active_for(&Rc<Interpreter>) -> bool`, `PromiseDriver::has_active_roots() -> bool`, and `ensure_promise_admission(&PromiseDriver) -> Result<(), &'static str>`.

- [ ] **Step 1: Attach interpreter identity to the legacy session**

Import `Weak`, add the owner, and install it with the submitted session:

```rust
use std::rc::{Rc, Weak};

struct DebugSession {
    owner: Weak<sema_eval::Interpreter>,
    debug: sema_vm::DebugState,
    handle: sema_vm::runtime::RootHandle,
}

fn legacy_debug_active_for(interp: &Rc<sema_eval::Interpreter>) -> bool {
    DEBUG_SESSION.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|session| session.owner.upgrade())
            .is_some_and(|owner| Rc::ptr_eq(&owner, interp))
    })
}
```

When legacy `debugStart` submits its root, store `owner: Rc::downgrade(&self.inner)`.

Add `DebugSession::is_owned_by` and use it in every legacy session operation. A foreign `debugStart` returns `status: "error"` before calling `self.debug_stop`; foreign continue/poll return the existing no-session error, stop and breakpoint updates are no-ops, locals returns null, stack returns an empty array, and `debugIsActive` returns false.

Classify weak owners as `Same`, `Foreign`, or `Dead`. Evict and cancel `Dead` sessions before admission, and add `Drop for WasmInterpreter` to cancel/remove the wrapper's owned legacy session. Unit-test that a dead weak owner is not classified as foreign.

Represent the global slot as `Starting`, `Active`, or `Driving`. Reserve `Starting` before parse/expansion and recheck after replacement cleanup and expansion. Move the `DebugSession` out of the slot for `runtime.drive`, leaving a `Driving` owner/root reservation with a reentrant stop flag, then restore or cancel after the drive returns. Browser-test registered JavaScript callbacks from both expansion and a debugged native call.

- [ ] **Step 2: Add exact Promise-driver admission predicates**

In `driver.rs`, add:

```rust
const LEGACY_DEBUG_CONFLICT: &str =
    "Promise-driven execution cannot start while the synchronous debugger is active on this interpreter";

impl PromiseDriver {
    pub(crate) fn has_active_roots(&self) -> bool {
        !self.promises.borrow().is_empty()
            || self.debug_root.get().is_some()
            || !self.retiring_debug_roots.borrow().is_empty()
    }
}

pub(crate) fn ensure_promise_admission(
    driver: &PromiseDriver,
) -> Result<(), &'static str> {
    if crate::legacy_debug_active_for(&driver.interp) {
        Err(LEGACY_DEBUG_CONFLICT)
    } else {
        Ok(())
    }
}
```

- [ ] **Step 3: Guard Promise source, adoption, and debugger admission**

At the beginning of `submit`, reject before `submit_str` when `ensure_promise_admission` fails. At the beginning of `adopt`, cancel the already-submitted handle, reject, schedule the driver once to apply cancellation, and return:

```rust
if let Err(message) = ensure_promise_admission(driver) {
    handle.cancel(sema_core::runtime::CancelReason::HostStop);
    reject_with_message(&reject, message);
    schedule_drive(driver);
    return;
}
```

Guard `debug_start_promise` in `lib.rs` before resetting counters, parsing, or expansion, and retain the same defensive check inside `driver::start_debug` before replacing a Promise debug session or submitting its VM.

- [ ] **Step 4: Guard compiled archive submission before deserialization**

In the bytecode branch of `run_entry_async`, before `push_file_path`, `deserialize_from_bytes`, or `submit_compile_result`, return the compatibility error object when admission fails:

```rust
if let Err(message) = driver::ensure_promise_admission(&self.promise_driver) {
    return self.eval_error_result(&SemaError::eval(message));
}
```

Keep the `adopt` check as a defensive invariant for direct internal callers.

- [ ] **Step 5: Guard legacy debugStart before any mutation**

Make the first statement in legacy `debug_start`:

```rust
if self.promise_driver.has_active_roots() {
    return self.debug_error_str(
        "the synchronous debugger cannot start while Promise-driven execution is active on this interpreter",
    );
}
```

Only after this check may the method call `self.debug_stop()`, clear output, parse, or expand macros.

- [ ] **Step 6: Format and run the RED tests to verify GREEN**

Run:

```bash
cargo fmt -- crates/sema-wasm/src/lib.rs crates/sema-wasm/src/driver.rs
cd playground
npx playwright test tests/unified-runtime.spec.ts --grep 'legacy debugger excludes|Promise root excludes|legacy debugger on interpreter A'
```

Expected: all three tests pass.

### Task 3: Focused verification and review handoff

**Files:**
- Verify: `crates/sema-wasm/src/lib.rs`
- Verify: `crates/sema-wasm/src/driver.rs`
- Verify: `playground/tests/unified-runtime.spec.ts`

**Interfaces:**
- Consumes: the completed admission implementation.
- Produces: a focused commit ready for the same independent reviewer.

- [ ] **Step 1: Run the Promise debugger coexistence gate**

Run:

```bash
cd playground
npx playwright test tests/promise-debugger.spec.ts --grep 'promise debugger stops and cancels only its target root'
```

Expected: PASS, proving ordinary `evalPromise` and Promise debugging still coexist.

- [ ] **Step 2: Run focused Rust and browser gates**

Run:

```bash
cargo test -p sema-wasm
cargo clippy -p sema-wasm --all-targets -- -D warnings
cd playground
npx playwright test tests/unified-runtime.spec.ts tests/promise-debugger.spec.ts
```

Expected: every command exits 0.

- [ ] **Step 3: Inspect scope and commit only owned files**

Run:

```bash
git diff --check -- crates/sema-wasm/src/lib.rs crates/sema-wasm/src/driver.rs playground/tests/unified-runtime.spec.ts
git add crates/sema-wasm/src/lib.rs crates/sema-wasm/src/driver.rs playground/tests/unified-runtime.spec.ts
git diff --cached --check
git diff --cached --stat
git commit -m "fix(wasm): coordinate debugger root admission"
```

Do not stage `crates/sema-vm/src/runtime/state.rs`, playground application/WebMCP files, website files, or any other pre-existing worktree changes.

- [ ] **Step 4: Stop for independent re-review**

Report the commit hash, RED/GREEN evidence, focused gate results, and preserved foreign files to the parent reviewer. Do not begin another slice until the same reviewer returns a verdict.

### Task 4: Symmetric Promise preparation reservation

**Files:**
- Modify: `crates/sema-eval/src/eval.rs`
- Modify: `crates/sema-wasm/src/driver.rs`
- Modify: `crates/sema-wasm/src/lib.rs`
- Modify: `crates/sema/tests/host_api_test.rs`
- Modify: `playground/tests/unified-runtime.spec.ts`

- [x] Add Playwright RED cases in which registered JavaScript functions re-enter same-interpreter legacy `debugStart` from `evalPromise` and `debugStartPromise` macro expansion. Assert rejection before root/macro creation, exact outer behavior, later reuse, and foreign-interpreter isolation.
- [x] Add a per-interpreter scoped Promise preparation counter. Include preparations in legacy admission, support nesting, and release through RAII on every return or unwind.
- [x] Add `Interpreter::submit_str_guarded` so the Promise source path rechecks after user-code-capable expansion and immediately before runtime submission. Recheck adoption and Promise-debugger submission at their handoff boundaries.
- [x] Add native coverage for nested/unwinding reservations and guarded submission ordering, then rebuild WASM and run the focused Rust, Playwright, and clippy gates.
