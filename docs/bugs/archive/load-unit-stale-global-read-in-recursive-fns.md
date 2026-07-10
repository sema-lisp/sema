# Stale global reads in recursive functions from `load`ed units

**Status:** FIXED (2026-07-10, issue #82). See "Fix" below.
**Found:** 2026-07-09, while hardening `examples/sema-coder` on 1.30.0.
**Severity:** high — silently breaks the "loop until a callback flips a flag"
pattern, which is the standard shape of every event loop. It is why the
sema-coder TUI cannot exit via `/quit`, Ctrl-C, or Ctrl-D (worked when the TUI
shipped, so this regressed somewhere in the 1.29.x–1.30.0 window; the TUI now
carries an accessor-read workaround, see `tui.sema` `should-quit?`).

## Repro (9 lines)

```scheme
;; lib.sema
(define *sq* #f)
(define (quit!) (set! *sq* #t))
(define (advance i) (when (= i 2) (quit!)))   ;; name it `advance`, not `step`:
                                              ;; `step` collides with a prelude macro
(define (run)
  (let loop ((i 0))
    (advance i)
    (unless *sq* (loop (+ i 1))))   ;; direct global read — never sees the set!
  (println "exited ok"))

;; main.sema
(load "lib.sema")
(run)                               ;; hangs forever
```

`run` looped forever: the `set!` performed two calls down the stack was never
observed by the loop's direct read of `*sq*`.

## Characterization (all on 1.30.0, release build)

| Variant | Result |
| --- | --- |
| Exactly as above (named let, via `load`) | **hung** |
| Same file run directly (no `load`) | works |
| Loop reads the flag through an accessor fn `(sq?)` instead of directly | works |
| Plain self-recursion instead of named let (`(define (spin i) … (unless *sq* (spin …)))`) | **hung** |
| Non-tail self-recursion (`(+ 0 (spin …))`) | **hung** (overflowed; every frame read stale `#f`) |

So it was *not* specific to the named-let/SelfTailCall rewrite: any function in
a `load`ed unit that **directly reads a global** and **recurses** (tail or
non-tail) kept seeing the value from before a cross-function `set!`. Fresh
invocations (the accessor variant) saw the new value — the write itself landed;
the in-flight recursive reader is what went stale.

## Root cause

The VM's inline global cache (`LOAD_GLOBAL`) is keyed on `(name, env.version)`:
a cached `(name, version, value)` triple is served as long as
`self.globals.version` is unchanged. `set!` (`STORE_GLOBAL`) bumps the version
of the env it writes through, invalidating the cache.

`Env::version` was a per-handle `Cell<u64>`, but `bindings` is a shared `Rc`.
`Env` derives `Clone`, so `env.clone()` shared the bindings map yet **forked the
version cell**. `eval_module_body_vm` runs each top-level form of a `load`ed
unit on its own per-form VM over `Rc::new(env.clone())`, so every function
defined in the unit captured a *different* home-globals handle: same bindings,
independent version cell. `run` resolved `*sq*` against handle **A**; `quit!`'s
`set!` bumped handle **B**. A's version never moved, so the loop's cache entry
`(*sq*, A.version, #f)` kept hitting and served `#f` forever.

- Single-file works: all top-level closures capture `self.globals.clone()`,
  which is an `Rc::clone` of the one base-globals `Env` — one shared version cell.
- The accessor variant "worked" only by accident: `sq?` and `quit!` collide on
  the same shared `inline_cache` slot, so an unrelated read clobbered the stale
  entry and forced a re-read. Layout-sensitive, hence fragile.

This is the same family as the 1.29.x "frame-dynamic globals left stale by
run_nested_closure" HOF fix — a cloned-Env handle diverging from the table
`set!` writes through — surfacing on the `load` path.

## Fix

Make the version counter travel *with* the bindings: `Env::version` is now an
`Rc<Cell<u64>>` (`crates/sema-core/src/value.rs`). Cloning an `Env` handle now
shares one version cell across all handles to the same bindings map, so a `set!`
through any handle is observed by a cache entry keyed on any other handle —
recursive reader and cross-function writer are back in sync by construction. A
genuinely new scope (`Env::new` / `Env::with_parent`) still gets its own cell.
The change is strictly safe: sharing a cell can only cause *more* cache
invalidations (re-reads), never a stale hit. The now-redundant explicit
`env.bump_version()` after `eval_module_body_vm` was removed.

Regression test:
`crates/sema/tests/integration_test.rs::test_load_recursive_fn_reads_live_global_after_cross_fn_set`
(keeps the exact fragile repro shape and bounds a regressed hang with an eval
step limit, so a reintroduction errors fast instead of wedging CI).

## Notes

- `sema-coder` structure that hit it: `main.sema` `load`s `tui.sema`; the
  key loop `(unless *should-quit* (loop))` never saw `run-command!`'s
  `(set! *should-quit* #t)` even though instrumentation proved the `set!` ran.
- Historical workaround for user code (no longer needed): read the flag via a
  helper function, or keep the flag in a `mutable-cell`.
