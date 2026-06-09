# Plan: Replacing Thread-Local Effects With EvalContext Handler Stacks

## Verdict

**Feasible but not as a sweeping refactor.** Worth doing as a **single, narrow first slice** (OutputSink), which simultaneously satisfies Decision #58. Continue case-by-case from there; do **not** preemptively rework all thread-locals.

Confidence: medium-high.

Strongest counterargument: of ~20 `thread_local!` sites, only ~3 are scope-shaped effects; the rest are process-global caches or VM-internal scratch where a handler stack adds nothing but a `RefCell::borrow_mut` per access. The "beautiful architecture trap" is real.

**Genuinely benefit (migrate to EvalContext handler stack):**
- Stdout/stderr capture (notebook `gag::BufferRedirect`, WASM `OUTPUT`/`LINE_BUF`, Decision #58) — this is the one true effect with composition value
- LLM `BUDGET_STACK` (already a stack! already per-eval-scoped — move it onto ctx)
- `STDIN_EOF` flag (per-eval, not process)

**Stay as-is:** HTTP runtime/client, sqlite connections, KV stores, serial ports, TTY store, signal callbacks, spinners, string-intern table, gensym counter, interner, scheduler TLS (VM-local by design), span map / lower depth (compile-time scratch), pricing cache, provider registry, `STDLIB_CTX` & `call_callback` (the cycle-break is fundamental — see below), `LAST_SOURCE` REPL state, fuzz ENV.

**Hybrid:** `YIELD_SIGNAL`/`RESUME_VALUE`/`IN_ASYNC_CONTEXT` — these are per-task and already coordinate with the scheduler; moving them to ctx is fine but mostly cosmetic.

**Smallest first slice:** Add `OutputSink` effect to `EvalContext` with RAII push/pop. Migrate notebook eval and WASM stdout to use it. Delete `gag` dependency. This proves the pattern, lands Decision #58, and is **fully reversible** if it doesn't pay off.

---

## 1. Verified inventory

Found via `rg "thread_local!" --type rust`. Excluding fuzz/test files:

### Genuine effects (per-eval, scope-shaped)

| # | Site | What it is | Composes? |
|---|------|-----------|-----------|
| A | `crates/sema-stdlib/src/io.rs:11` `STDIN_EOF` | EOF flag for stdin; sticky-per-eval | one-deep; per-eval reset would be fine |
| B | `crates/sema-wasm/src/lib.rs:10-14` `OUTPUT` + `LINE_BUF` | WASM stdout capture buffer | one-deep, but conflated with backend |
| C | (Decision #58, proposed) `OUTPUT_WRITER` in `io.rs` | hookable stdout sink | **explicitly wants nesting** (notebook cell within test harness, etc.) |
| D | `crates/sema-llm/src/builtins.rs:39` `BUDGET_STACK` (and the four flat `BUDGET_*` cells it shadows) | per-`with-budget` cost cap | **already a stack — clearly an effect** |
| E | `crates/sema-core/src/async_signal.rs:52` `YIELD_SIGNAL`, `RESUME_VALUE`, `IN_ASYNC_CONTEXT` | one-shot per-call signal between native fn and VM | strictly one-deep (one yield per native call) |

### Process-global resources (should stay)

| # | Site | Why TLS makes sense |
|---|------|---------------------|
| 1 | `crates/sema-stdlib/src/http.rs:6` `HTTP_RUNTIME`, `HTTP_CLIENT` | connection-pool / runtime; lifetime = process |
| 2 | `crates/sema-stdlib/src/sqlite.rs:7` `DB_CONNECTIONS` | open DB handles by name |
| 3 | `crates/sema-stdlib/src/kv.rs:11` `KV_STORES` | named persistent stores |
| 4 | `crates/sema-stdlib/src/serial.rs:9` `PORTS`, `NEXT_ID` | open serial port handles |
| 5 | `crates/sema-stdlib/src/io.rs:17` `TTY_STORE`, `TTY_COUNTER` | OS termios tokens — global resource |
| 6 | `crates/sema-stdlib/src/terminal.rs:37` `SPINNERS`, `SPINNER_COUNTER` | external threads, opaque handles |
| 7 | `crates/sema-stdlib/src/system.rs:17` `SIGNAL_CALLBACKS` | OS signal handlers, async-signal-safe only |
| 8 | `crates/sema-stdlib/src/system.rs:277` `START` | clock origin |
| 9 | `crates/sema-stdlib/src/string.rs:10` `STRING_INTERN_TABLE` | interner |
| 10 | `crates/sema-core/src/value.rs:22` `INTERNER` (lasso `Rodeo`) | global interner — by design |
| 11 | `crates/sema-core/src/value.rs:59` `GENSYM_COUNTER` | unique-symbol monotonic counter |
| 12 | `crates/sema-llm/src/pricing.rs:29` `CUSTOM_PRICING`, `FETCHED_PRICING` | pricing tables; process-global |
| 13 | `crates/sema-llm/src/builtins.rs:29,59` `PROVIDER_REGISTRY`, `CACHE_*`, `RATE_LIMIT_*`, `VECTOR_STORES`, `LISP_PROVIDERS`, `FALLBACK_CHAIN`, `PRICING_WARNING_SHOWN`, `EVAL_FN`, `SESSION_USAGE`, `LAST_USAGE` | mostly process-global LLM infra |
| 14 | `crates/sema-vm/src/scheduler.rs:248` `SCHEDULER` | VM-internal; take/put dance is intentional |
| 15 | `crates/sema-vm/src/lower.rs:13,267` `LOWER_DEPTH`, `SPAN_MAP`, `COUNTER` | compile-time scratch |
| 16 | `crates/sema-eval/src/special_forms.rs:138` `SF` (interned spurs cache) | startup lazy-init |
| 17 | `crates/sema-wasm/src/lib.rs:18,26,28,38` `VFS`, `VFS_DIRS`, `VFS_TOTAL_BYTES`, `HTTP_CACHE`, `DEBUG_SESSION` | WASM-runtime singletons |
| 18 | `crates/sema/src/main.rs:122` `LAST_SOURCE`, `LAST_FILE` | REPL session state |
| 19 | `crates/sema-core/src/context.rs:524` `STDLIB_CTX` | bootstrap for Decision #6 — see §4 |

### Architectural callbacks (not really "effects" but related)

- `EvalCallbackFn`, `CallCallbackFn`, `SpawnCallbackFn`, `RunSchedulerCallbackFn`, `CancelCallbackFn` — registered once at interpreter startup, never popped. These are **not** effect handlers; they're a dependency-inversion plug for a cyclic build graph (Decision #6). The handler-registry pattern does not help here.

**Counting: ~3 of ~20 thread-locals are effect-shaped.** That's the disclosure the verdict hinges on.

---

## 2. Re-read of `EvalContext`

File: `crates/sema-core/src/context.rs`.

- Already threaded as `&EvalContext` into every `NativeFn::with_ctx` (line 88 of `value.rs`). `NativeFn::simple` exists for noise-reduction only and is trivially convertible.
- Already contains stack-shaped state: `current_file`, `module_load_stack`, `call_stack`, `user_context` (`Vec<BTreeMap>`), `hidden_context`, `context_stacks` (`BTreeMap<Value, Vec<Value>>`). So the **shape we'd add is already there**; this isn't a new architectural concept, it's an extension.
- Has `eval_deadline: Cell<Option<Instant>>` — note: this is currently *one slot, not a stack*. Nested `with-deadline` would clobber the outer. That's a real bug-in-waiting and a candidate for the same treatment (see §5).
- The shared `STDLIB_CTX` at line 524 is **the** wart: stdlib HOFs that take user functions but were registered as `simple` (no ctx) reach for this global to invoke the eval callback (`with_stdlib_ctx(|ctx| call_callback(ctx, &func, &[..]))` — see `io.rs` line 704). If every native fn with a callback argument became `with_ctx`, `STDLIB_CTX` could go away entirely. Worth quantifying.

---

## 3. The hard case: stdlib→eval callback (Decision #6)

`STDLIB_CTX` + `eval_fn`/`call_fn` callbacks exist because `sema-stdlib` cannot depend on `sema-eval` (`sema-eval` depends on `sema-stdlib` to register builtins). The cycle is real. Two facts:

1. The **callback indirection is fundamental.** Handler registries do not eliminate it. `sema-stdlib` must still call into `sema-eval` through a fn-pointer registered at startup.
2. The **`STDLIB_CTX` global is *not* fundamental.** It exists only because `NativeFn::simple` doesn't carry a ctx. If `register_fn` (the `simple` path) is removed in favor of `with_ctx` everywhere, `STDLIB_CTX` dies. That's a real win but it's a *separate, mechanical refactor* — not really part of effect handlers.

Recommendation: leave the callback fn-pointer pattern; do the `STDLIB_CTX` cleanup as an independent follow-up.

---

## 4. The hard case: WASM

- `thread_local!` in WASM compiles to a single static per logical thread; there is one "thread" in single-threaded WASM. So TLS isn't broken; it's just a regular global with the right initialization story.
- The handler-registry pattern works identically in WASM because it lives inside `EvalContext`, which is just a struct. No `wasm-bindgen-rayon`/threading required.
- **Improvement specifically for WASM:** today's `OUTPUT`/`LINE_BUF` is a parallel-but-divergent capture path from the notebook's `gag::BufferRedirect`. An `OutputSink` effect unifies them: WASM just pre-installs the sink at interpreter init; the notebook installs it per cell; everywhere else the sink is `None` and writes pass to `std::io::stdout()`. Net code reduction in WASM, not increase.

---

## 5. Compose-or-not

| Effect | Needs nesting? | Evidence |
|--------|---------------|----------|
| OutputSink | **Yes** | "notebook cell within `cargo test`", or future `(with-output-to-string ...)` Lisp form; Decision #58 explicitly anticipates this |
| BudgetStack | **Yes** | Already a `Vec<BudgetFrame>` (`builtins.rs:39`) — proves nesting needed |
| EvalDeadline | **Yes** | Currently a `Cell<Option<Instant>>` — only one-deep — should become a stack so nested `with-timeout` works |
| StdinEof | No | Single flag; per-eval reset suffices |
| YieldSignal | No | One-shot signal; depth-1 by construction |

For non-nesting effects, a single `RefCell<Option<T>>` slot is enough; no need for `Vec<Box<dyn Handler>>`. Don't over-engineer.

---

## 6. Decision #58 interaction

This plan **subsumes** Decision #58, not just generalizes it. Decision #58 proposes "a thread-local writer hook in `crates/sema-stdlib/src/io.rs`". Land Phase 1 of this plan *instead*: put the writer hook on `EvalContext`, not in stdlib TLS. Mark #58 as superseded.

---

## 7. Proposed EvalContext shape (sketch)

Sketch only — no implementation. Add to `EvalContext`:

```rust
// In crates/sema-core/src/context.rs

pub trait OutputSink {
    fn write(&self, s: &str) -> std::io::Result<()>;
    fn write_err(&self, s: &str) -> std::io::Result<()> { /* default: stderr passthrough */ }
    fn flush(&self) -> std::io::Result<()>;
}

pub struct EvalContext {
    // ... existing fields ...
    output_sink: RefCell<Vec<Rc<dyn OutputSink>>>,    // top-of-stack wins; empty = real stdout
    deadline_stack: RefCell<Vec<Instant>>,            // replaces eval_deadline Cell
    // (budget moves out of sema-llm thread-local into here, optional Phase 3)
}

// RAII guard — drops automatically on scope exit even in panic paths.
#[must_use]
pub struct OutputSinkGuard<'a> { ctx: &'a EvalContext }
impl Drop for OutputSinkGuard<'_> {
    fn drop(&mut self) { self.ctx.output_sink.borrow_mut().pop(); }
}

impl EvalContext {
    pub fn push_output_sink(&self, sink: Rc<dyn OutputSink>) -> OutputSinkGuard<'_> {
        self.output_sink.borrow_mut().push(sink);
        OutputSinkGuard { ctx: self }
    }
    pub fn write_stdout(&self, s: &str) -> std::io::Result<()> {
        if let Some(top) = self.output_sink.borrow().last() {
            return top.write(s);
        }
        use std::io::Write;
        std::io::stdout().write_all(s.as_bytes())
    }
}
```

Notebook usage:
```rust
let buf = Rc::new(VecSink::new());
{
    let _g = ctx.push_output_sink(buf.clone());
    interpreter.eval_str_compiled(source)
} // _g drops here, sink popped automatically
let captured = buf.take();
```

WASM `println` / `display` / `print` / `newline` all redirect through `ctx.write_stdout(..)`; the WASM entrypoint pre-pushes a `WasmLineBufferSink` once at boot and never pops. Eliminates `OUTPUT`/`LINE_BUF` thread-locals (or makes them implementation detail of `WasmLineBufferSink`).

---

## 8. Migration matrix

| Thread-local | Decision | Rationale |
|---|---|---|
| Notebook `gag::BufferRedirect` (`engine.rs:187`) | **Migrate** | Phase 1; the keystone case |
| WASM `OUTPUT` / `LINE_BUF` | **Migrate** | Same Phase 1; collapses two stdout paths into one |
| Decision #58 proposed `OUTPUT_WRITER` | **Replace with this plan** | Mark #58 superseded |
| `STDIN_EOF` | **Migrate** (cheap) | Move into ctx as `Cell<bool>`; resets per top-level eval naturally |
| `BUDGET_STACK` (sema-llm) | **Migrate (Phase 3, optional)** | Already a stack; lifting it to ctx removes the cross-crate TLS coupling and exposes "current budget" to the notebook UI |
| `eval_deadline` (existing) | **Convert to stack** (Phase 2) | Today it's a one-slot Cell — nested `with-deadline` is broken; same effort as adding OutputSink stack |
| `YIELD_SIGNAL`, `RESUME_VALUE`, `IN_ASYNC_CONTEXT` | **Keep TLS, defer** | Single-slot, hot-path, intricate VM/scheduler dance; moving them is mostly cosmetic and risks subtle bugs |
| `HTTP_CLIENT`, `HTTP_RUNTIME` | **Keep TLS** | Process-global connection pool |
| `DB_CONNECTIONS`, `KV_STORES`, `PORTS`, `TTY_STORE`, `SPINNERS`, `SIGNAL_CALLBACKS` | **Keep TLS** | Each is a named-handle resource registry |
| `INTERNER`, `GENSYM_COUNTER`, `STRING_INTERN_TABLE` | **Keep TLS** | Inherently global identity |
| `PROVIDER_REGISTRY`, pricing, cache, rate-limit | **Keep TLS** | Process-scoped LLM infra |
| `SCHEDULER` (VM) | **Keep TLS** | VM-internal; take/put dance is intentional |
| `LOWER_DEPTH`, `SPAN_MAP`, lower `COUNTER`, `SF` | **Keep TLS** | Compile/lowering-time scratch; outside eval |
| WASM `VFS`, `VFS_DIRS`, `HTTP_CACHE`, `DEBUG_SESSION` | **Keep TLS** | WASM-runtime singletons |
| `LAST_SOURCE`, `LAST_FILE` (REPL) | **Keep TLS** | REPL UI state |
| `STDLIB_CTX` + `eval_fn`/`call_fn` registrations | **Keep**, optionally trim | Cycle-break is fundamental; `STDLIB_CTX` itself can die later by collapsing `simple` → `with_ctx` |

**Score: ~3 migrate, ~1 hybrid stack-conversion, ~16 stay.** This matches the verdict.

---

## 9. Cross-cutting concerns this unblocks

- **`(with-output-to-string e ...)`** as a real Sema form. Today impossible without the OS-level `gag` trick.
- **Per-cell timeout that nests** (e.g. notebook cell sets 5s, user code wraps a sub-eval in 1s). The current single-Cell deadline silently overwrites; the stack fixes it.
- **LLM budget visible from the notebook UI mid-stream** (currently buried in `sema-llm` TLS; moving to ctx lets the engine read it without reaching across crate boundaries).
- **Parallel notebook eval** (future Decision; not today). Each cell's `EvalContext` owns its own sinks/deadlines; no global FD race.

What this does **not** unblock realistically: tracing/profiling (already lives on ctx via `call_stack` + would need a `TraceSink` you'd add the same day either way); per-eval LLM cost budgets (BUDGET_STACK already works).

---

## 10. Blast radius

Phase 1 (OutputSink only):
- `crates/sema-core/src/context.rs` — add fields, methods, guard.
- `crates/sema-stdlib/src/io.rs` — re-route `display`, `print`, `println`, `pprint`, `newline`, `print-error`, `println-error`, `io/flush` through `ctx.write_stdout`. These are currently `register_fn` (= `simple`); they must become `with_ctx`. Mechanical.
- `crates/sema-notebook/src/engine.rs:187` — replace `gag::BufferRedirect` with `ctx.push_output_sink(...)`. Remove `gag` dependency from `Cargo.toml`.
- `crates/sema-wasm/src/lib.rs` — replace `OUTPUT`/`LINE_BUF` with a `WasmLineBufferSink` pushed once at interpreter init.

Public API impact: **none** at the Sema-language level. Internal: `NativeFn::simple` versions of the print fns become `with_ctx`. Most callers use these via the macro, so the diff is small.

Phase 2 (deadline-stack): pure additive on `EvalContext`.

Phase 3 (BudgetStack lift): touches `sema-llm/src/builtins.rs` BUDGET_* TLS and the `sema-core` `EvalContext`. Larger diff (~200 lines). Optional. Only do this if there's a concrete demand (e.g. notebook UI wants budget readout).

Performance: `ctx.output_sink.borrow().last()` is a `RefCell` borrow + `Vec::last`. Hot path is `println` — not actually hot in typical Sema code. Acceptable. If it ever measures hot, swap the `RefCell<Vec>` to a `Cell<u32> + RefCell<Vec>` fast-path-on-empty, or to `&'static dyn` for the always-installed WASM sink.

---

## 11. Phasing

**Phase 1 — OutputSink (the proving ground).** ~1 day.
- Add `OutputSink` trait, stack, guard to `EvalContext`.
- Migrate stdout-writing stdlib fns to `with_ctx`.
- Replace notebook `gag` usage.
- Replace WASM `OUTPUT`/`LINE_BUF`.
- Drop `gag` from `sema-notebook/Cargo.toml`.
- Mark Decision #58 superseded.

**Phase 2 — Deadline stack.** ~0.5 day. Convert `eval_deadline` Cell to `Vec<Instant>` (use min-deadline-wins semantics). Add `push_deadline` returning a guard. Audit existing callers (notebook engine).

**Phase 3 — Budget stack lift (optional, deferred).** ~2 days. Move BUDGET_STACK out of `sema-llm` TLS onto `EvalContext`. Requires either making `sema-core` know about budget (ugly) or defining an `EffectSlot` keyed by `TypeId` so individual crates can install their own effect types. The latter is clean but introduces dynamic dispatch + `Any` downcasts. Decision to be made when Phase 1+2 are landed.

**Phase 4 — STDLIB_CTX retirement (optional, independent).** Convert remaining `register_fn` callers that touch `with_stdlib_ctx` to `register_fn_with_ctx`. Delete `STDLIB_CTX` + `with_stdlib_ctx`. Pure cleanup, not really part of "effect handlers" but the obvious follow-up.

**Stop after Phase 2 unless a concrete future feature demands Phase 3.**

---

## 12. Risks

- **Ergonomics:** more native fns become `with_ctx`. The diff in `io.rs` alone is ~10 functions. Acceptable. Most non-IO fns stay `simple`.
- **Performance:** `RefCell` borrow on each `println`. Not on a hot path; verify with bench if paranoid.
- **Rust's lack of effect-handler language support:** This is *the* big design pressure. No `try ... with` syntax means every effect site needs an explicit `if let Some(handler) = ctx.foo.last()` check. With only 2–3 effects that's fine; with 15 it becomes wallpaper noise. **Don't grow this past Phase 2.**
- **`Rc<dyn OutputSink>` vs `Box<dyn>`:** sinks are pushed by the engine, written-to from many native fns. `Rc` is the right call (cheap clone for the borrow-from-top trick).
- **WASM regression risk:** WASM stdout testing is in the WASM crate's own tests. Make sure they still pass; `take_output()` semantics must match exactly.
- **`gag` removal subtlety:** today's `gag::BufferRedirect` captures *all* stdout including native code that bypasses Sema's print fns (rare but possible — e.g. a future `eprintln!` debug print). The new sink only captures Sema-level output. This is **explicitly endorsed** by Decision #58 ("That's actually correct"), but it is a behavioral change worth flagging in the changelog.

---

## 13. Effort estimate

| Phase | Effort | Confidence |
|---|---|---|
| 1 — OutputSink | 1 dev-day | high |
| 2 — Deadline stack | 0.5 dev-day | high |
| 3 — Budget lift | 2 dev-days | medium |
| 4 — STDLIB_CTX retirement | 1 dev-day | high |

Cumulative if all four are done: ~4.5 days. **Stop after Phase 1+2 (≈1.5 days) unless concrete demand emerges.** That is the recommendation.

---

## Critical files

- `crates/sema-core/src/context.rs`
- `crates/sema-stdlib/src/io.rs`
- `crates/sema-notebook/src/engine.rs`
- `crates/sema-wasm/src/lib.rs`
- `crates/sema-core/src/value.rs`
