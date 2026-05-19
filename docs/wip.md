# Work in progress — open threads from the May 2026 quality sweep

Status board for follow-up work that has been planned, partially explored, or that needs a design call before code can land. Each entry has enough context that you (or a future you / agent) can pick it up months from now without re-reading the audit.

For items that have been *parked* with a clear "we're not doing this", see `docs/deferred.md`. For items that are deemed shipped, see commits `09f6c21`, `01f250c`, `a5b37df`, `c2b551b`, `6c4086f`, `83cbd58`.

---

## Wave 6b — Performance papercuts

### P2 — `lower_list` does 40+ thread-local interner borrows per call site

`crates/sema-vm/src/lower.rs:131-211`. Every list-form being lowered (i.e. every function call in source code) checks against ~40 special-form names. Each check calls `sf("name")` which is `INTERNER.with(borrow_mut().get_or_intern(...))` — that's a `RefCell::borrow_mut` plus a hashbrown lookup per name per call.

**Proposed fix:** cache all 40 special-form Spurs once at startup via `static SPECIAL_FORMS: OnceLock<SpecialForms>` where `SpecialForms` is a struct of `Spur` fields (`quote: Spur, if_: Spur, …`). The hot match becomes a ~40-way `==` chain on Spur ids. Or better: a `HashMap<Spur, SpecialFormKind>` lookup once per call, then dispatch on the enum.

**Why not done:** measurable but compile-time-only impact. Would help startup and macroexpand cost on large source files; doesn't affect steady-state VM dispatch. Worth doing when we next profile compile time.

**Bench needed:** new bench `lower-throughput` — time to lower a synthetic 10k-form file. Add to `examples/benchmarks/`.

### P6 — `partition` / `frequencies` / `list/group-by` double-clone

`crates/sema-stdlib/src/list.rs:340-352, 502-510, 427-444`. Even after P1, these still clone twice per element (once for the callback args, once when pushing into the output bucket). Changing the loop from `for item in &items { ... matching.push(item.clone()) }` to `for item in items.iter().cloned() { let keep = ...; if keep { matching.push(item) } else { ... } }` would save one clone per element.

**Why not done:** P1 showed Rc::clone is too cheap to measure on a HOF-dispatch-bound benchmark. Same logic applies — won't move the needle. Revisit if a profile shows partition/group-by as a hotspot.

### P7 — `CALL_NATIVE` clones `Rc<NativeFn>` per call

`crates/sema-vm/src/vm.rs:942-944` (now ~979 after Wave 6a):
```rust
let native = self.native_fns[native_id].clone();   // Rc<NativeFn> bump
let args_start = self.stack.len() - argc;
let result = (native.func)(ctx, &self.stack[args_start..]);
```

The clone exists solely to release the borrow on `self.native_fns` so we can also borrow `self.stack`. Two ways out:

1. Take a raw pointer: `let native_ptr: *const NativeFn = &*self.native_fns[native_id]; let func = unsafe { &(*native_ptr).func };` — safe because `self.native_fns` is never mutated during dispatch. Adds an `unsafe` block but eliminates the bump.
2. Refactor `NativeFn` so the function pointer is `Copy` (a bare `fn` ptr or `Box<dyn Fn>`'s `&dyn Fn`) and stash it in a parallel `Vec<...>` keyed by id.

**Why not done:** medium effort + needs careful borrow-checker dance + introduces unsafe. Per-stdlib-call cost is one Rc bump (not Arc); single-threaded codebase means it's cheap. On a microbench with `samply` on a stdlib-heavy workload it might show up; not on the current bench suite.

---

## Wave 6c — Design-then-code (need an ADR before fixing)

### D3 — `(match …)` should error on no-match

`crates/sema-eval/src/special_forms.rs:1813-1814` falls through with `Trampoline::Value(Value::nil())` when no clause matches. This is a Sussman-class lurking-bug — a non-exhaustive `match` is almost always a bug, and silently returning nil masks it.

**Proposed fix:** make the canonical `match` raise `:match-failed`. Introduce `match*` (or `match-or-nil`) for the lenient form. Update both backends + dual-eval tests.

**Why this needs a design call:** breaking-ish behavior change. Anyone whose code currently relies on the nil fall-through (probably some) would now error. We need to decide:
- Strict default + permissive variant (breaks existing code, safer long-term)
- Permissive default + strict variant `match!` (no break, but the safer behavior is opt-in)
- Compiler-warning on non-exhaustive `match` patterns we can statically detect (no runtime change, alerts users)

The first option is the right long-term call but needs a deprecation period and a changelog entry.

### N9 — Numeric domain policy

Inconsistent today (verified):
- `(/ 1 0)` → error "division by zero"
- `(sqrt -1)` → returns `NaN`
- `(pow 2 -1)` → silently float-promotes to `0.5`
- `(pow 0 0)` → returns `1`

Three different error policies (raise / NaN sentinel / silent promotion) for similar domain failures.

**Proposed fix:** pick a uniform policy and document it. Two reasonable choices:
- "Raise on all undefined domains" (Scheme-ish, predictable, breaks anyone relying on NaN)
- "Always return IEEE specials; raise only on integer-divide-by-zero" (Python-ish, less predictable, doesn't break)

**Why this needs a design call:** language-level decision. Document in `docs/adr.md` and `website/docs/stdlib/math.md`. Implementation is small once decided.

---

## Notes on what's NOT here

- **Items that shipped:** see commits `09f6c21` (Wave 1) through `83cbd58` (Wave 6a) — git log + the per-wave commit messages have the full list.
- **Items that are formally parked:** see `docs/deferred.md`. Each entry there explains *why* we won't fix and what the workaround is.
- **C1 (VM upvalue model)** and **C11 (.semac stack-balance verifier):** see `docs/limitations.md` items #31/#32 and `docs/adr.md` ADRs #55/#56. Multi-week design work; tracked there, not here.
- **#57 (source spans through runtime errors)** and **#58 (thread-local writer hook):** also in `docs/adr.md`. Pulled out of Wave 2 as too big for that pass.

---

## Recently shipped — removed from WIP

- **P1 — `get_sequence` borrows instead of clones** — shipped in commit `49c4c5c` ("perf(stdlib): borrow input slice in HOF helper (Wave 6b/P1)"). `get_sequence` now returns `&[Value]`; verified in `crates/sema-stdlib/src/list.rs` (call sites iterate by reference, `sort` etc. use `.to_vec()`).
- **A1 + A4 + D2 — Async semantics pass** — shipped in commit `72c8308` ("feat(async): A1+A4+D2 — FIFO scheduler, async/cancel returns bool, Cancelled state variant"). See `CHANGELOG.md` 1.15.0 under "Changed (async semantics pass — A1 + A4 + D2)".
