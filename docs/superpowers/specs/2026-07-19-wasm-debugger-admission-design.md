# WASM debugger admission coordination

The synchronous legacy debugger and the Promise-driven runtime must never own roots on the same interpreter at the same time. They use different driving protocols: the legacy debugger drives the whole runtime synchronously, while the Promise driver selects only roots in its ownership tables. Admitting roots to both protocols can therefore reject a JavaScript Promise while leaving its runtime root alive to execute later.

## Ownership boundary

Each legacy `DebugSession` records a weak reference to its `Interpreter`. Admission checks compare interpreter identity, not the thread-global session slot alone. A paused legacy debugger on interpreter A must not block Promise work on interpreter B.

The legacy session slot remains single-session for compatibility, but every legacy operation verifies ownership. Start on interpreter B rejects while A owns the slot; continue, poll, stop, locals, stack trace, active-state, and breakpoint calls on B act as though B has no session. They cannot resume, cancel, inspect, replace, or mutate A's session.

Dropping a `WasmInterpreter` cancels and removes its legacy session. Admission also distinguishes a live foreign owner from a dead `Weak`: a stale session is cancelled and evicted before a new legacy root may be submitted, so garbage collection cannot permanently reserve the thread-local slot or leave a live root to be overwritten.

The Promise driver exposes whether it owns any ordinary, active-debug, or retiring-debug roots. This includes all roots that its selected drive may need to settle or retire.

## Admission rules

Promise source evaluation, adoption of an already-submitted compiled root, and Promise debugger start reject when a legacy debug session owns the same interpreter.

Legacy `debugStart` rejects when the same interpreter's Promise driver owns any root. The check runs before stopping an existing legacy session, clearing output, parsing, macro expansion, compilation, or root submission. A rejected start therefore cannot mutate macro state or disturb the active owner.

Ordinary `evalPromise` roots and the Promise debugger continue to coexist. The exclusion applies only across the legacy and Promise driving protocols.

Compiled archive execution checks admission before deserializing and submitting its root. `adopt` also checks defensively. If an already-submitted root reaches a rejected adoption, it is cancelled and the Promise driver schedules a turn so it cannot become an orphan.

## Error behavior

Promise evaluation rejects its JavaScript Promise with an error that identifies the synchronous debugger conflict. Promise debugger and legacy debugger APIs retain their existing result-object convention and return `status: "error"`.

The checks are synchronous and atomic in the browser's single-threaded WASM execution model. No lease or runtime-global lock is required.

## Regression coverage

Browser tests cover:

1. A legacy debugger paused before `evalPromise`: the Promise rejects before reporting a root, and its body never mutates state after the legacy debugger resumes.
2. A pending `evalPromise` before legacy `debugStart`: the legacy start rejects before expansion or submission, the Promise settles once, and no rejected debugger body runs later.
3. A legacy debugger on interpreter A while interpreter B runs `evalPromise`: B proceeds normally and A remains independently resumable.
4. Legacy debugger operations on interpreter B cannot observe, replace, stop, or mutate interpreter A's session.
5. Existing Promise-debugger concurrency tests remain green.
