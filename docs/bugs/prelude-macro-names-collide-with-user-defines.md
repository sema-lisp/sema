# Prelude macro names cannot be defined or shadowed — expansion rewrites define heads and call sites

**Status:** open (pre-existing; found by the CORE-2 M3 GC stress campaign, 2026-07-02; initially misdiagnosed as a nested-define lowering bug)
**Verified against:** installed `sema 1.28.1` and the `worktree-core2-gc-design` build
**Area:** macro expander + `sema-eval` prelude (`crates/sema-eval/src/prelude.rs`), NOT `lower.rs`/`resolve.rs`, not the collector

## Repro

```bash
sema -e '(define (step n) n) (println (step 3))'
# Error: Eval error: define: expected a symbol
```

`step` is a prelude workflow macro (`crates/sema-eval/src/prelude.rs:228`).
Macro expansion runs before lowering and rewrites **any** list whose head
names a macro — including the head of define sugar. `(step n)` expands to the
macro's `(let ((st-opts0# {}) (st-prompt# n)) …)` template, so the define
lowering sees `(define (let ((…)) …) …)` and rejects the binding-list
"params" with `define: expected a symbol` (`crates/sema-vm/src/lower.rs:311`).

The same collision breaks every binding shape — nesting, `let` bodies, and
lambda wrappers are all irrelevant (each of these fails with the identical
error; renaming `step` → `stp` makes every one pass):

```bash
sema -e '(define (outer a) (define (step n) (let ((v 1)) v)) (step 3)) (println (outer 1))'
sema -e '(define (outer a) (fn () (define (step n) n) (step 3))) (println ((outer 1)))'
sema -e '(define (outer a) (fn () (define (step n) (let ((v 1)) v)) (step 3))) (println ((outer 1)))'
# all: Error: Eval error: define: expected a symbol
```

Non-sugar defines and parameters don't help: local bindings cannot shadow a
macro at a **call site** either, because expansion is not scope-aware:

```bash
sema -e '(define step (fn (n) n)) (println (step 3))'
sema -e '(define (call step) (step 3)) (println (call (fn (n) (* n 2))))'
# both: Type error: expected string or prompt, got int   (the workflow/step runtime)
```

Only non-head positions are safe: `(define step 42)` followed by a bare
`step` reference works.

## Failure modes vary by macro shape

- `step` expands to a `let` template → loud compile error (above).
- `phase` / `checkpoint` expand to a plain call whose head is another symbol
  (`workflow/phase`, `workflow/checkpoint`) → `(define (phase n) n)`
  **silently defines `workflow/phase`** instead, clobbering the workflow
  runtime's binding for the rest of the session, while `(phase 3)` appears
  to work because its expansion now calls the user's function:

```bash
sema -e '(define (phase n) n) (println (workflow/phase 3))'
# 3        (workflow/phase has been replaced by the user's identity fn)
```

At-risk prelude names users are likely to reach for: `step`, `phase`,
`checkpoint`, `parallel`, `pipeline`, `dotimes`, `for-range`.

## Notes

- Regression coverage: `eval_test.rs`
  (`prelude_macro_name_in_define_sugar_head_errors` pins the collision;
  `nested_define_with_let_*` pin that the same shapes compile fine with a
  non-macro name — there is no define/`let`/lambda nesting bug in
  `lower.rs`/`resolve.rs`).
- Proper fix is a language-level decision: either treat define-sugar heads
  (and other binding positions) as non-expansion positions, or make macro
  lookup scope-aware so lexical bindings shadow macros (R7RS keyword
  shadowing / Clojure locals-shadow-macros semantics). The first is a small
  targeted change but leaves call-site collisions (`(step 3)`) broken; only
  the second removes the trap entirely.
- Found while writing GC stress workloads: a helper named `step` inside a
  lambda failed to compile, which looked shape-dependent (nesting/`let`)
  until every control shape was actually executed with the colliding name —
  all fail identically, so the shape is irrelevant. Diagnosis rule this
  earned: rename the identifier before blaming the compiler.
