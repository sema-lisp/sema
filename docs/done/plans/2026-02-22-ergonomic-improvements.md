# Ergonomic Improvements — Tracking

**Issue:** https://github.com/HelgeSverre/sema/issues/6
**Date:** 2026-02-22

---

## Priority Order (effort vs gain)

### Phase 1 — Quick Wins

- [x] **1. String interpolation `f"..."`** ✅
  - **Effort:** Low | **Gain:** Very High
  - Reader macro: `f"Hello ${name}"` → `(str "Hello " name)`
  - Lexer tokenizes `f"..."` into `FString` token with `Literal`/`Expr` parts
  - Reader expands `FString` to `(str ...)` call via recursive `read()`
  - Supports: nested expressions `${(+ 1 2)}`, escape `\$`, `$` without `{` is literal
  - Files changed: `crates/sema-reader/src/lexer.rs`, `crates/sema-reader/src/reader.rs`
  - Also refactored escape handling into shared `read_string_escape()` function
  - 16 reader unit tests + 8 integration tests added

- [x] **2. Auto-load threading macros** ✅
  - **Effort:** Very Low | **Gain:** High
  - Created `crates/sema-eval/src/prelude.rs` with `->`, `->>`, `as->`, `some->`
  - Auto-loaded at interpreter startup via `load_prelude()` in both `new()` and `new_with_sandbox()`
  - 7 integration tests added

- [x] **3. Promote `when-let` / `if-let` to builtins** ✅
  - **Effort:** Very Low | **Gain:** Medium-High
  - Added `when-let` and `if-let` macros to the prelude (auto-loaded)
  - 4 integration tests added

### Phase 2 — Low Effort, High Value

- [x] **4. Add `get-in` / `update-in` / `assoc-in` / `deep-merge`** ✅
  - **Effort:** Low | **Gain:** High
  - `get-in` — nested access with optional default, nil-safe
  - `assoc-in` — nested set, creates intermediate maps if missing
  - `update-in` — nested update with function
  - `deep-merge` — recursive merge of nested maps (variadic)
  - Files changed: `crates/sema-stdlib/src/map.rs`
  - 12 integration tests added

- [x] **5. Short lambda `#(* % %)`** ✅
  - **Effort:** Low-Medium | **Gain:** High
  - Lexer emits `ShortLambdaStart` token on `#(`
  - Reader parses body, scans for `%`/`%1`/`%2`, rewrites `%` → `%1`
  - Expands to `(lambda (%1 %2 ...) body)` at parse time
  - Skips recursion into nested `(lambda ...)` / `(fn ...)` forms
  - Files changed: `crates/sema-reader/src/lexer.rs`, `crates/sema-reader/src/reader.rs`
  - 7 reader unit tests + 6 integration tests added

### Phase 3 — Medium Effort, Very High Value

- [x] **6. Destructuring in `let` / `define` / `lambda`** ✅
  - **Effort:** Medium-High | **Gain:** Very High
  - New `crates/sema-eval/src/destructure.rs` module with `destructure()` and `try_match()` entry points
  - Vector destructuring `[a b & rest]` — exact or rest-args, nested patterns supported
  - Map destructuring `{:keys [a b]}` — binds keyword keys to symbols, nil for missing
  - Explicit map key-pattern pairs `{:key pattern}` also supported
  - Wired into `let`, `let*`, `define` (all accept patterns in binding position)
  - Lambda parameter destructuring via desugaring: `(lambda ([a b] {:keys [x]}) ...)` → temp args + `let*` wrapper
  - Files changed: `crates/sema-eval/src/destructure.rs` (new), `crates/sema-eval/src/special_forms.rs`, `crates/sema-eval/src/lib.rs`

- [x] **7. Pattern matching `match`** ✅
  - **Effort:** High | **Gain:** Very High
  - New special form: `(match expr [pattern body...] ...)`
  - Supports: literals, symbol binding, wildcards `_`, vector patterns, map patterns (structural + `:keys`), quoted literals
  - Guards via `when`: `[pattern when (> x 0) body...]`
  - Shares `destructure.rs` infrastructure (`try_match()` for soft matching)
  - TCO-compatible: tail-calls last body expression in matched clause
  - Returns `nil` when no clause matches
  - Also added `Value::is_falsy()` convenience method to `sema-core`
  - Files changed: `crates/sema-eval/src/special_forms.rs`, `crates/sema-core/src/value.rs`

### Backlog — Lower Priority

- [x] **8. Regex literals `#"..."`** ✅
  - **Effort:** Low | **Gain:** Low-Medium
  - Reader macro: `#"\\d+"` → string `\d+` (raw, no escape processing except `\"`)
  - Added `'"'` arm in lexer's `#` match — emits `Token::String` (no new type)
  - Files changed: `crates/sema-reader/src/lexer.rs`
  - 5 reader unit tests + 6 dual-eval integration tests added

- [x] **9. REPL `*1` / `*2` / `*3` / `*e` history** ✅
  - **Effort:** Very Low | **Gain:** Low
  - `*1`, `*2`, `*3` hold last 3 results; `*e` holds last error message
  - Shifts on each successful eval, sets `*e` on error
  - Updated `,help` with history variables section
  - Files changed: `crates/sema/src/main.rs`

- [x] **10. `spy` / `time` / `assert` / `assert=` debug helpers** ✅
  - **Effort:** Very Low | **Gain:** Low-Medium
  - `spy` — `(spy "label" value)` prints `[label] value` to stderr, returns value
  - `time` — `(time thunk)` calls thunk, prints elapsed ms to stderr, returns result
  - `assert` — `(assert condition)` or `(assert condition "msg")` throws on falsy
  - `assert=` — `(assert= expected actual)` throws on inequality with diff message
  - Files changed: `crates/sema-stdlib/src/meta.rs`
  - 12 dual-eval tests added (8 success + 4 error)

- [x] **11. Multimethods `defmulti` / `defmethod`** ✅
  - Effort: Medium | Gain: Medium
  - New `MultiMethod` value type with interior-mutable dispatch table (NaN-boxed, TAG 24)
  - `defmulti` creates a multimethod with a dispatch function; directly callable
  - `defmethod` adds handlers for specific dispatch values; `:default` sets fallback
  - Open extension: add methods from anywhere without touching existing code
  - Dispatch on any value: keywords, types, integers, computed multi-arg values
  - Files changed: `crates/sema-core/src/value.rs`, `crates/sema-core/src/lib.rs`, `crates/sema-eval/src/special_forms.rs`, `crates/sema-eval/src/eval.rs`, `crates/sema-vm/src/lower.rs`
  - 24 dual-eval tests added (16 success + 8 error)

- [ ] **12. Keyword arguments `&keys`** — Deferred
  - Effort: Medium | Gain: Low
  - Options-map pattern + existing destructuring covers most use cases

---

## Completed

_(Items move here as they're implemented)_

---

## Notes

- All Phase 1 items can be done without touching the evaluator core
- Phase 2 items need minor evaluator/reader changes
- Phase 3 items are the biggest effort but highest long-term payoff — now complete
- Destructuring (item 6) is prerequisite for pattern matching (item 7) — both done
- Items 8-12 are nice-to-have and can be done opportunistically
- **Still needs:** docs/website updates
- **Dual-eval test migration:** 1,077 tests across 9 test files ensure TW/VM equivalence covering: core language (arithmetic, closures, control flow, error handling, macros, TCO), destructuring, match, f-strings, short lambdas, threading macros, when-let/if-let, data types (char, bytevector, base64, bitwise, delay/force, records, embeddings), map ops (get-in/assoc-in/update-in/deep-merge), JSON, regex, CSV, format, hash, type predicates/conversions, string ops, list ops, vector ops, text processing, terminal/ANSI, pretty-print, context system, prompt/message/conversation/document primitives, tool/agent definitions, LLM utilities, file I/O, path ops, system/env, time, shell, and more
