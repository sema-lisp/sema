# Critical Fixes — Code Review Findings

Date: 2026-02-17

## Summary

A critical review of Sema's internals revealed several issues ranging from security holes to undefined behavior risks. This document tracks all findings, prioritized by severity.

## 🔴 Critical (Fixed)

### 1. Sandbox bypass in VM delegates
**Status: FIXED**

`__vm-load` and `__vm-import` called `std::fs::read_to_string` directly without checking `ctx.sandbox.check(Caps::FS_READ, ...)`. Running `--vm --sandbox=strict` allowed arbitrary file reads.

**Fix:** Added `ctx.sandbox.check(Caps::FS_READ, "load")` and `ctx.sandbox.check(Caps::FS_READ, "import")` to both VM delegate functions in `eval.rs`.

### 2. VM opcode dispatch uses magic numbers instead of `Op` enum
**Status: FIXED**

The VM `run()` loop matched on raw `u8` values (`0`, `1`, `2`, ...) with comments like `0 /* Const */`. The `Op` enum existed with a `from_u8()` method but wasn't used. Any opcode renumbering would silently break the VM.

**Fix:** Replaced all 42 magic number arms with `Op` enum constants (e.g., `Op::Const as u8 =>`). The compiler now catches any mismatch.

### 3. NaN-boxing has no compile-time platform guard
**Status: FIXED**

`ptr_to_payload` had `debug_assert!(raw >> 48 == 0)` but this only fires in debug builds. On platforms with >48-bit virtual addresses, pointers would silently truncate in release mode → UB.

**Fix:** Added `const _: ()` compile-time assertion in `value.rs` that refuses to compile on non-64-bit platforms or platforms where pointer width exceeds what NaN-boxing can encode.

### 4. VM uses `unsafe` for no measurable benefit
**Status: FIXED**

The dispatch loop used `unsafe { get_unchecked(fi) }` and a raw pointer cast to borrow the code slice. If `frames` was ever empty (error path), this would be UB. The pointer cast was completely unnecessary — the borrow already existed.

**Fix:** Replaced `unsafe { get_unchecked(fi) }` with safe indexing. Removed the raw pointer cast. The code slice is now borrowed safely.

## 🟠 Serious (Tracked — Future Work)

### 5. Thread-local callback architecture
**Status: FIXED**

`set_eval_callback`/`set_call_callback` were global TLS. Creating two `Interpreter` instances in one thread silently overwrote callbacks. **Fix:** Moved callbacks into `EvalContext` as `Cell<Option<fn>>` fields. `STDLIB_CTX` kept for simple-fn stdlib closures.

### ~~6. LLM domain types in `sema-core`~~ — NOT A PROBLEM
~~`Prompt`, `Conversation`, `Agent`, `ToolDefinition` in core `Value`.~~ Reconsidered: LLM primitives are Sema's defining feature, not a bolted-on extension. These types need to be first-class values. The NaN-boxing tag space has room (~15 of 64 tags used). Moving them out would add indirection with no practical benefit.

### 7. String interner leaks forever
**Status: MITIGATED**

`INTERNER` is TLS, never evicts. Not fixable without replacing `lasso::Rodeo`. **Mitigation:** Added `interner_stats()` function and `sys/interner-stats` builtin returning `{:count N :bytes N}` for monitoring.

### 8. Span table keyed by `Rc` pointer address
**Status: FIXED (partial)**

Pointer-address keys can theoretically be reused after dealloc. The hard-clear at `MAX_SPAN_TABLE_ENTRIES` was replaced with a soft limit that skips new spans when full, preserving existing error locations. The fundamental pointer-reuse issue remains but is low-risk in practice.

## 🟡 Significant (Tracked — Quality)

### 9. No cross-mode tests (tree-walker vs VM)
**Status: FIXED**

Added 11 new `assert_equiv` tests: delay/force, nested maps, variadic arithmetic, deep tail recursion, bytevectors, records, quasiquote, try/catch error types, mutual recursion, and apply. Threading macros (`->`, `->>`) skipped as they require macro imports. Total: 64 cross-mode tests.

### 10. Stdlib silent error swallowing
**Status: FIXED**

`file/list` and `file/glob` now propagate IO errors instead of silently dropping them via `.filter_map(|e| e.ok())`.

### 11. `try` catches all error types
**Status: DOCUMENTED**

This is a deliberate design choice — `error_to_value` already converts each error type to a map with a `:type` keyword that users can discriminate on. Added documentation to the special-forms page with a table of all 9 error `:type` values and a re-throw pattern example.

### 12. `transmute::<u32, Spur>` is fragile
Appears in multiple places. If `lasso` changes internal representation, this breaks silently. Should use safe conversion APIs.
