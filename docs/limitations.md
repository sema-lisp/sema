# Sema Lisp — Known Limitations & Gaps

Assessed against standard Scheme (R7RS) and practical Lisp expectations.
Status: originally written as of v0.5.0; verified and updated against v1.16.0 on 2026-06-09.

---

## ~~Critical — Blocks Practical Programming~~ (RESOLVED in Phase 6)

### ~~1. No Error Handling~~ → RESOLVED

Implemented `try`/`catch`/`throw` in Phase 6.

### ~~2. No Module System~~ → RESOLVED

Implemented `module`/`import` in Phase 6.

---

## ~~High — Significantly Limits Usability~~ (RESOLVED in Phase 6+7)

### ~~3. No Named `let`~~ → RESOLVED (Phase 6)

### ~~4. No `letrec`~~ → RESOLVED (Phase 6)

### ~~5. No `eval` / `read` / `string->symbol`~~ → RESOLVED (Phase 7)

`eval` special form, `read`/`read-many` builtins, `string->symbol`, `symbol->string`, `string->keyword`, `keyword->string`.

### ~~6. No `gensym` or `macroexpand`~~ → RESOLVED (Phase 7)

`gensym` with optional prefix, `macroexpand` for single-step expansion.

### ~~7. No Regular Expressions~~ → RESOLVED (Phase 7)

Full regex suite: `regex/match?`, `regex/match`, `regex/find-all`, `regex/replace`, `regex/replace-all`, `regex/split`.

### ~~8. Limited List Operations~~ → RESOLVED (Phase 6)

---

## Medium — Nice to Have

### ~~9. `do` Aliased to `begin`~~ → RESOLVED

Proper Scheme `do` loop implemented. `begin` remains for sequencing.

```scheme
(do ((i 0 (+ i 1)) (sum 0 (+ sum i)))
    ((= i 10) sum))  ; => 45
```

### ~~10. No `case` / Pattern Matching~~ → RESOLVED

`case` special form with R5RS semantics. Pattern matching also shipped since: `match` special form (map patterns `{:name n}`, vector/list patterns `[a b c]`, literals, `_` wildcard) plus destructuring (`crates/sema-eval/src/destructure.rs`).

### ~~11. No Port-Based I/O~~ → MOSTLY RESOLVED

Full file namespace: `file/read`, `file/write`, `file/append`, `file/delete`, `file/rename`, `file/list`, `file/mkdir`, `file/info`, `file/exists?`, `file/is-directory?`, `file/is-file?`, `file/is-symlink?`.
Path ops: `path/join`, `path/dirname`, `path/basename`, `path/extension`, `path/absolute`.
HTTP: `http/get`, `http/post`, `http/put`, `http/delete`, `http/request`.
Streaming shipped since: `stream/*` namespace (`crates/sema-stdlib/src/stream.rs` — read/write, byte ops, from-string/from-bytes, buffers, copy/flush/close, backed by the `SemaStream` trait) and incremental line streaming over files (`file/read-lines`, `file/for-each-line`, `file/fold-lines`). Remaining gap: no `file/open` returning a stream handle — streams are string/byte-buffer/server-backed, so port-style file handles are still absent.

### ~~12. No Struct/Record Types~~ → RESOLVED

R7RS `define-record-type` with constructors, type predicates, and field accessors. `record?` predicate. `type` returns record type name as keyword.

### ~~13. No Stack Traces~~ → PARTIAL (VM)

The stack-trace machinery — call frames, file locations, and source spans, bounded for TCO'd recursion — is now **implemented** on the VM. Caught error maps include a `:stack-trace` field (list of `{:name :file :line :col}` frame maps), and inline opcodes (`+`, `-`, `car`, etc.) produce synthetic intrinsic frames. Source spans are threaded through the main eval path via `compile_program_with_spans_and_natives`.

### ~~14. Missing Math Functions~~ → RESOLVED (Phase 8)

Full math suite: `math/quotient`, `math/remainder`, `math/gcd`, `math/lcm`, `math/tan`, `math/asin`, `math/acos`, `math/atan`, `math/atan2`, `math/exp`, `math/log10`, `math/log2`, `math/random`, `math/random-int`, `math/clamp`, `math/sign`.
Bitwise: `bit/and`, `bit/or`, `bit/xor`, `bit/not`, `bit/shift-left`, `bit/shift-right`.

### ~~15. No `guard` (R7RS Style)~~ → RESOLVED

R7RS `(guard (var clause ...) body ...)` is implemented (as a prelude macro over `try`/`catch`), alongside the R7RS `raise` procedure. Clauses are tried like `cond` (with optional `else`); when no clause matches, the condition is re-raised to the enclosing handler rather than swallowed. `guard` catches both `(raise obj)` and native runtime errors. The pre-existing `try`/`catch`/`throw` with `:type`-keyed error maps remains available too.

---

## Low — Completeness Only

### ~~17. No Char Type~~ → RESOLVED

Full character type: `#\a`, `#\space`, `#\newline` literals. `char?`, `char->integer`, `integer->char`, `char-alphabetic?`, `char-numeric?`, `char-whitespace?`, `char-upper-case?`, `char-lower-case?`, `char-upcase`, `char-downcase`, `char->string`, `string->char`, `string->list`, `list->string`. `string-ref` and `string/chars` now return `Char` values.

### 18. No Continuations

No `call/cc` or `call-with-current-continuation`. The trampoline evaluator cannot capture continuations.

### ~~19. No Multiple Return Values~~ → RESOLVED

`values`, `call-with-values`, `let-values`, `let*-values`, `define-values`. A
multi-value bundle is represented internally as a `Record` tagged
`%multiple-values%` (see `crates/sema-stdlib/src/list.rs`) — no VM/opcode
changes were needed. `(values x)` (exactly one value) is identity, so a single
value flows through ordinary contexts (`(+ 1 (values 2))` works) exactly as
if `values` weren't there; only `call-with-values` (and the `let-values`
family built on it) inspects the bundle. Leaking a zero/multi-value bundle
into a plain single-value context is unspecified by R7RS; Sema currently
prints it as an opaque `#<record %multiple-values% …>` rather than spreading
it. Because `call-with-values` dispatches producer/consumer through the same
native `call_function` boundary as `apply`, that call is not a true VM tail
call — deep recursion written through `let-values`/`call-with-values` won't
get the same TCO as a plain named-let (same limitation as other stdlib HOF
callbacks, #24).

### ~~20. No Dynamic Binding~~ → PARTIAL (`parameterize`/`make-parameter` RESOLVED)

`make-parameter` / `parameterize` are implemented in the prelude (a parameter
is a closure over a mutable cell; `parameterize` installs converted values and
restores the prior raw values via the same try/catch/throw-rethrow-then-
restore idiom as `with-stream`/`with-retry`, so restoration also happens
across a raised condition). Still missing: `dynamic-wind` itself, and
`parameterize`'s restore is unwind-on-error only — Sema has no `call/cc`, so
there is no true continuation-based unwind. If a `parameterize` body suspends
via an async park (an `AwaitIo` yield) rather than returning or raising, the
`try` does not observe the suspension: the parameter stays bound across the
yield and can leak into sibling tasks until the body resumes. Single-shot
synchronous dynamic scoping is correct; cross-yield dynamic scoping is out of
scope.

### 21. Hygienic Macros — `syntax-rules` (partial)

`(define-syntax name (syntax-rules (literals...) (pattern template)...))` is
supported: pattern matching with literal identifiers and ellipsis (`...`),
first-match-wins rule ordering, an optional custom ellipsis symbol, and template
expansion. Hygiene is **binder-directed** and backed by the gensym engine that
also powers `foo#` auto-gensym: a pass over the winning rule's template collects
exactly the identifiers the template introduces *as binders* (the vars a
template-introduced `let` / `let*` / `letrec[*]` / `lambda` / `fn` / `define` /
`do` / named-let binds), and only those are consistently alpha-renamed to a
fresh gensym per expansion. Every other template identifier — free references to
user-defined globals, builtins, special forms, and the macro's own name for
recursion — is kept verbatim and resolves at the use site / runtime. So a
template's own introduced bindings cannot capture user identifiers of the same
name, while a template can still freely reference user-defined globals (this
works even in whole-program mode, where macros are pre-expanded before the
user's `define`s run).

Known limitations of the approximation:

- Hygiene is binder-directed, not built on per-identifier **definition**
  environments. Referential transparency against a use-site that *shadows* a
  special form/global that the template references (the rarer "other direction")
  is not covered.
- Nested ellipsis is supported in patterns (e.g. `((name val) ...)`), and single
  ellipsis in templates; a template with ellipsis depth > 1 (`x ... ...`) is
  rejected with a clear error rather than mis-expanded.
- `define-syntax` is registered by the macro pre-expansion pass, so — like
  `defmacro` — a `define-syntax` nested inside a lambda/let body (not at the top
  level or inside a top-level `begin`) is not visible to sibling forms.
- `syntax-case` is not supported.

### 22. No Tail Position in `do` Body

The `do` loop evaluates its body for side effects but does not support tail calls within the body — only the result expressions are in tail position.

### 23. No `string-set!`

Strings are immutable. No in-place character mutation.

### 24. No Proper Tail Recursion in `map`/`filter`

Higher-order list functions use Rust iteration internally, not Scheme-level recursion — correct but not extensible via tail calls.

### ~~25. No `char=?` / `char<?` Comparison Predicates~~ → RESOLVED

Full R7RS character comparison: `char=?`, `char<?`, `char>?`, `char<=?`, `char>=?` and case-insensitive `char-ci=?`, `char-ci<?`, `char-ci>?`, `char-ci<=?`, `char-ci>=?`.

### 26. No `with-exception-handler`

Only `try`/`catch`/`throw`. No R7RS `with-exception-handler` / `raise` / `raise-continuable`.

### ~~27. No `define-values`~~ → RESOLVED

See #19 — `define-values` desugars to a `begin` of plain `define`s over a
gensym'd temp holding the produced values (`nth`/`drop` to pick them apart).

### ~~28. No Bytevectors~~ → RESOLVED

`Value::Bytevector` with `#u8(1 2 3)` reader syntax. `make-bytevector`, `bytevector`, `bytevector-length`, `bytevector-u8-ref`, `bytevector-u8-set!` (COW), `bytevector-copy`, `bytevector-append`, `bytevector->list`, `list->bytevector`, `utf8->string`, `string->utf8`, `bytevector?`.

### ~~29. No `let-values`~~ → PARTIAL RESOLVED

`let-values` (parallel) and `let*-values` (sequential) are implemented — see
#19. SRFI-8 `receive` is not (a thin macro over `call-with-values`; not yet
added since it's not R7RS-required).

### ~~30. No Tail Calls Across Mutual Recursion in Stdlib~~ → RESOLVED (folded into #24)

The mini-eval this entry referenced was deleted (callback architecture, ADR #61); HOF callbacks now run through the full evaluator and mutual tail recursion inside a callback TCOs fine (verified at depth 500k on the VM). The only remaining issue is #24's: a callback that *re-enters* a stdlib HOF recursively grows the Rust stack.

---

## Known Backend Bugs (Audit Findings)

### 31. VM `set!` through stdlib HOF callbacks loses the mutation (C1, HIGH) — FIXED 2026-06-18

**Resolved.** When a closure captures a let-bound variable and that closure is invoked via a stdlib higher-order function (`map`, `filter`, `for-each`, `sort-by`, `foldl`, `retry`, etc.), `set!` performed inside the closure now propagates back correctly on the VM.

```
$ sema -e '(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)'
6
```

Root cause (was): the open-upvalue runtime (ADR #55, `UpvalueState::{Open,Closed}` in `vm.rs`) closed open upvalue cells **before every non-VM call**, then ran the HOF callback via `NativeFn::func` on a *fresh* VM (Decision #50). The closed cells were detached snapshots, so the callback's `set!` never reached the parent's live stack slot.

Fix (route HOF callbacks in-VM): the VM no longer closes upvalues before native calls. Instead it registers itself on a thread-local `CURRENT_VM` stack for the duration of each native call (`CurrentVmGuard`). When a VM closure's `NativeFn` fallback fires synchronously, it consults that stack and — if a compatible VM is running — executes the closure as a **nested frame on that live VM** (`run_nested_closure`, bounded by `frame_floor`). The closure's open upvalue cells therefore stay connected to the parent's stack and `set!` flows back. Closures that genuinely cross onto a *foreign* stack (async task VMs created by `spawn` / inline HOF tasks) are snapshotted at the crossing point via `close_closure_upvalues_for_foreign_run`. Full write-up: `docs/bugs/vm-set-lost-through-hof-callbacks.md`; plan: `docs/plans/2026-06-18-c1-vm-hof-in-vm.md`. Regression tests: `crates/sema/tests/eval_test.rs` (`hof_set_through_*`, `vm_set_through_*`).

Still-open related symptoms from the same dual-path design (NOT addressed here):

- `(type (fn (x) x))` returns `:native-fn` on the VM, since VM closures wrap as `NativeFn` for stdlib HOF interop (deferred — TW-2 in `docs/deferred.md`).
- ~~Caught error maps on the VM are missing `:stack-trace` (deferred — TW-1 in `docs/deferred.md`; see #13).~~ **Fixed** — caught errors now include `:stack-trace`.

Note: `set!`-through-HOF inside an *async task* still follows the pre-existing fresh-task-VM semantics (it is snapshotted on spawn), since async tasks run on dedicated VM stacks.

### 32. Bytecode stack-balance validation gap (C11, HIGH)

The VM's main dispatch loop uses `pop_unchecked` at ~67 call sites (`crates/sema-vm/src/vm.rs`). This is safe **only** because the in-process bytecode compiler is stack-balanced by construction (every emitted sequence pushes/pops by a known delta). The on-disk `.semac` format has no such guarantee: `validate_bytecode` (in `crates/sema-vm/src/serialize.rs`) currently checks magic, version, table bounds, and jump targets, but it does **not** abstract-interpret the instruction stream to verify stack balance.

A hand-crafted (or corrupted) `.semac` file with a leading `Pop`, an unbalanced `Call`, or a missing push before a binary op causes undefined behavior in release builds: `pop_unchecked` reads `stack[len - 1]` after subtracting from an empty `Vec`, calls `set_len(usize::MAX)`, and subsequent pushes/pops corrupt arbitrary memory.

**For now, `.semac` files should be treated as trusted-source-only.** Do not load `.semac` from network/untrusted sources without verification. The planned fix is a stack-depth verifier — see the ADR "Bytecode stack-depth verifier for .semac loading" in `docs/adr.md` and the implementation plan `docs/plans/2026-05-15-adi-bytecode-verifier.md`.

### 33. VM `eval` sees globals only — no lexical locals

On the VM, `(eval expr)` (the `__vm-eval` native) macro-expands, compiles, and runs the form against the **global environment only**. Lexical locals are not visible: `(define (f x) (eval 'x)) (f 42)` → `Unbound variable: x`. The "reify locals as a read-only view" design (archived `docs/decisions.md`, eval-reify section) was never implemented — the name→slot tables it required exist (`Function::local_names`) but are used only for DAP variable inspection.

### 34. `import`/`load` that shadows a builtin isn't seen by same-program compiled calls

A program compiled as one unit bakes calls to *known builtins* into `CallNative` opcodes (the `known_natives` optimization, resolved at compile time). A top-level `(define (truncate ...) ...)` is handled correctly (the compiler sees the define and skips `CallNative`), but a redefinition that happens at **runtime** via `(import ...)` / `(load ...)` is invisible to the compiler, so later calls in the same program still dispatch to the original builtin:

```
;; module exports a 2-arg `truncate`; the call still hits the 1-arg math builtin
(import "string-utils") (truncate "hello" 3)  ; => Arity error: truncate expects 1 args, got 2
```

This worked on the retired tree-walker (late name resolution). On the VM, avoid shadowing a builtin name from an imported/loaded module (rename it, e.g. `truncate-str`), or `define` the override at top level in the same program. A general fix (conservatively disabling `CallNative` for programs containing `import`/`load`) was rejected for the per-call perf cost. The same compilation-unit boundary applies in the other direction to the intrinsic opcodes and the constant folder: a top-level `(define not ...)` (or any foldable/intrinsic name — `+`, `car`, `=` …) in one unit is invisible to code compiled in a *different* unit (a `(load ...)`ed file, a later REPL line), which still dispatches to — or folds with — the builtin.

### 35. Eager bulk allocators can exhaust memory (resource DoS, found by fuzzing)

`(range 0 5277777779992)` and similar eager bulk-allocating builtins try to materialize the entire result up front, so a single call with an absurd size exhausts memory and aborts the process — the per-eval step limit doesn't help because it's one native call, not a loop the VM counts. Surfaced by the `fuzz_eval` VM fuzzer (an out-of-memory artifact, not a panic/UB — the VM itself stayed memory-safe across the run). This is a denial-of-service concern for untrusted input / sandboxed embedding, not a soundness bug. Possible fixes (deferred): a configurable element/allocation cap that errors instead of OOMing, or lazy ranges. Until then, validate sizes before calling `range` on untrusted input.

---

### 36. Special-form names win over local bindings in operator position

The bytecode lowerer (`crates/sema-vm/src/lower.rs`) is **scope-free**: it resolves a special form (`if`, `fn`/`lambda`, `let`, `and`, `or`, `cond`, `define`, `quote`, `match`, `message`, …) from the head symbol of a call *before* it knows anything about local bindings. So a local binding whose name collides with a special form **shadows fine in value position** but **cannot override that form in operator/head position** — there, the special form silently wins:

```sema
(let ((message "hi")) message)        ; => "hi"   (value position — fine)
(defun api-error (code message) ...)  ; => ok     (message is just a value)
(let ((and (fn (a b) (* a b)))) (and 3 4))  ; => 4 (the `and` special form, NOT 12)
```

The last line is the footgun: `and` in head position is the special form, not the lambda. This is rare in practice (you'd have to shadow a special-form name *and* call it as an operator).

**History — a reservation was tried and reverted.** A bind-site check that *rejected* binding any special-form name (ADR #65) was shipped in 1.20.4 and **reverted in 1.21.2**: it was too aggressive — it broke common, correct value-position code (a function parameter named `message`, a variable named `fn`, etc.) to prevent the rare operator-position case, because the scope-free lowerer can't tell value use from operator use. The reservation also slipped a CI regression past four releases (it broke repo example files).

**Proper fix (future work):** make local bindings shadow special forms *everywhere*, including operator position — Scheme's hygienic model — by threading lexical scope through lowering so it "just works" without the user thinking about it. Until then, this is a documented, accepted footgun; pinned by `special_form_wins_in_operator_position` in `eval_test.rs`.

---

## Gap Analysis — Remaining Items

| #   | Gap                                      | Priority | Effort    | Notes                                                                        |
| --- | ---------------------------------------- | -------- | --------- | ---------------------------------------------------------------------------- |
| 15  | No `guard` (R7RS)                        | Low      | Low       | `try`/`catch` covers the use case; `guard` is syntactic sugar                |
| 18  | No Continuations                         | Low      | Very High | Requires CPS transform or VM rewrite; trampoline can't capture continuations |
| 20  | No Dynamic Binding                       | Low      | Medium    | `parameterize`/`make-parameter` via thread-local state                       |
| 21  | Hygienic Macros (`syntax-rules`) partial | —        | —         | Shipped: pattern/ellipsis/literals + binder-directed hygiene; see §21 for caveats |
| 22  | No Tail Position in `do` Body            | Low      | Low       | Body is for side effects; result exprs already have TCO                      |
| 23  | No `string-set!`                         | Low      | Low       | Intentional — immutable strings are simpler and safer                        |
| 24  | No Proper Tail Recursion in map/filter   | Low      | Medium    | Stdlib uses Rust iteration; would need eval access                           |
| 26  | No `with-exception-handler`              | Low      | Medium    | `try`/`catch` is sufficient for most use cases                               |
| 29  | No `receive` (SRFI-8)                    | Low      | Low       | Thin macro over `call-with-values`; `let-values` covers the use case         |
| 33  | VM `eval` sees globals only              | Medium   | High      | Reify design never built; lexical locals not reified for `eval`              |

---

## Recommended Next Implementations

1. **Dynamic binding** (#20) — `make-parameter`/`parameterize` via thread-local storage fits the existing architecture.
2. **`guard`** (#15) — Low effort syntactic sugar over `try`/`catch`.

---

## What Works Well

- **Closures** — Properly implemented with lexical scoping
- **Tail Call Optimization** — Trampoline-based, works for direct recursion in `if`/`cond`/`let`/`begin`/`and`/`or`/`when`/`unless` + named `let`
- **Data types** — Int, Float, String, Char, Symbol, Keyword, List, Vector, Map, Record, Bytevector, Bool, Nil, Promise + LLM types
- **Record types** — R7RS `define-record-type` with constructors, predicates, field accessors. `record?`, `type` returns record tag
- **Multiple values** — R7RS `values`, `call-with-values`, `let-values`, `let*-values`, `define-values`
- **Bytevectors** — `#u8(1 2 3)` literal syntax. `make-bytevector`, `bytevector`, `bytevector-length`, `bytevector-u8-ref`, `bytevector-u8-set!` (COW), `bytevector-copy`, `bytevector-append`, `bytevector->list`, `list->bytevector`, `utf8->string`, `string->utf8`
- **Character comparison** — R7RS `char=?`, `char<?`, `char>?`, `char<=?`, `char>=?` + case-insensitive `char-ci=?` etc.
- **Macros** — `defmacro` with quasiquote/unquote/unquote-splicing (non-hygienic but functional); R7RS `define-syntax`/`syntax-rules` with pattern/ellipsis matching and binder-directed hygiene (approximate — see §21)
- **String operations** — split, trim, replace, contains?, format, str, index-of, chars, repeat, pad-left, pad-right
- **Map operations** — hash-map, get, assoc, dissoc, keys, vals, merge, contains?, count, entries, map-vals, filter, select-keys, update
- **List operations** — car/cdr + 12 compositions (caar through cdddr), cons, map (multi-list), filter, foldl, foldr, reduce, sort, range, take, drop, zip, flatten, partition, any, every, member, last, apply, index-of, unique, group-by, interleave, chunk, assoc/assq/assv (alist lookup)
- **Async/concurrency** — `async`/`await`, channels, `async/sleep`, task scheduler with cooperative yield (ADR #53/#54)
- **Pattern matching** — `match` special form (map/vector/literal patterns, `_` wildcard) + destructuring
- **Streaming I/O** — `stream/*` namespace + `file/read-lines`/`file/for-each-line`/`file/fold-lines`
- **Lazy evaluation** — `delay`/`force` with memoized promises, `promise?`, `promise-forced?`
- **Iteration** — proper Scheme `do` loop with parallel assignment
- **Math** — Full suite: abs, min, max, floor, ceil, round, sqrt, pow, log, sin, cos, tan, asin, acos, atan, atan2, exp, log10, log2, gcd, lcm, quotient, remainder, random, random-int, clamp, sign, pi, e
- **Bitwise** — bit/and, bit/or, bit/xor, bit/not, bit/shift-left, bit/shift-right
- **Error handling** — try/catch/throw with typed error maps
- **Module system** — module/import with exports, selective import, caching, namespace isolation
- **File I/O** — `file/read`, `file/write`, `file/append`, `file/delete`, `file/rename`, `file/list`, `file/mkdir`, `file/info`, `file/exists?`, `file/is-directory?`, `file/is-file?`, `file/is-symlink?`
- **Path ops** — `path/join`, `path/dirname`, `path/basename`, `path/extension`, `path/absolute`
- **Regex** — `regex/match?`, `regex/match`, `regex/find-all`, `regex/replace`, `regex/replace-all`, `regex/split`
- **HTTP** — `http/get`, `http/post`, `http/put`, `http/delete`, `http/request`
- **Metaprogramming** — `eval`, `read`, `read-many`, `gensym`, `macroexpand`, `case`, type conversions
- **System** — env, shell, exit, time-ms, sleep, sys/args, sys/cwd, sys/platform, sys/env-all
- **JSON** — json/encode, json/decode, json/encode-pretty
- **Crypto/encoding** — uuid/v4, base64/encode, base64/decode, hash/sha256
- **Date/time** — time/now, time/format, time/parse, time/date-parts
- **CSV** — csv/parse, csv/parse-maps, csv/encode
- **REPL** — Line editing, history, multi-line input
- **LLM integration** — Full suite: complete, stream, chat, extract, classify, batch, pmap, embed, agents, tools, conversations, cost tracking, budgets
