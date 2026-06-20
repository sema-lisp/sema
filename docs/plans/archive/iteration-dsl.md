# Comprehensive Loop DSL — Design Research for Sema

> 📦 **ARCHIVED (2026-06-20) — design not adopted.** The full CL-style `loop`
> grammar (~50 keywords: FOR/COLLECT/ACROSS/UNTIL/…) explored here was never
> implemented. Sema shipped the simpler counted-iteration macros `dotimes` and
> `for-range` (in `crates/sema-eval/src/prelude.rs`) instead. Kept for historical
> context; revisit only if a comprehensive loop DSL is genuinely wanted.

> Deep investigation of CL-style `loop` iteration DSL: architecture, grammar, internals, and how to implement it in Sema.
> Status: **Research / Pre-Implementation Spec**

---

## Table of Contents

1. [The CL `loop` — Complete Grammar and Semantics](#1-the-cl-loop--complete-grammar-and-semantics)
2. [CL `loop` Internal Architecture](#2-cl-loop-internal-architecture)
3. [The `iterate` Library — A Better `loop`](#3-the-iterate-library--a-better-loop)
4. [Cross-Dialect Comparison](#4-cross-dialect-comparison)
5. [Sema Infrastructure for a Loop DSL](#5-sema-infrastructure-for-a-loop-dsl)
6. [Sema `loop` Design — Full Specification](#6-sema-loop-design--full-specification)
7. [Implementation Architecture](#7-implementation-architecture)
8. [Desugaring Rules](#8-desugaring-rules)
9. [Error Handling Strategy](#9-error-handling-strategy)
10. [Testing Strategy](#10-testing-strategy)

---

## 1. The CL `loop` — Complete Grammar and Semantics

The Common Lisp `loop` is a macro that implements a comprehensive iteration DSL with ~50 keywords. It is the most complex single macro in the CL standard, and its implementations range from 900 lines (CLISP) to 2000+ lines (SBCL/MIT loop).

### 1.1 Formal Grammar (from ANSI CL / CLtL2)

```
loop       ::= (loop [named NAME] {variable}* {main}*)
variable   ::= with | initial-final | for-as | repeat
main       ::= unconditional | accumulation | conditional
             | termination | initial-final

;; Variable initialization and stepping
with       ::= WITH var [type-spec] [= expr] {AND var [type-spec] [= expr]}*
for-as     ::= {FOR | AS} var [type-spec] iteration-driver {AND ...}*
repeat     ::= REPEAT expr

;; Iteration drivers (the core complexity)
iteration-driver ::=
    ;; Arithmetic
    | {FROM | UPFROM | DOWNFROM} expr [{TO | UPTO | DOWNTO | BELOW | ABOVE} expr] [BY expr]
    ;; List
    | IN expr [BY step-fn]
    | ON expr [BY step-fn]
    ;; Vector
    | ACROSS expr
    ;; Computed
    | = expr [THEN expr]
    ;; Hash-table
    | BEING {THE | EACH} {HASH-KEY | HASH-KEYS | HASH-VALUE | HASH-VALUES} {OF | IN} expr
          [USING ({HASH-KEY | HASH-VALUE} other-var)]
    ;; Package
    | BEING {THE | EACH} {SYMBOL | SYMBOLS | PRESENT-SYMBOL | EXTERNAL-SYMBOL} {OF | IN} expr

;; Accumulation
accumulation ::= {COLLECT | COLLECTING} expr [INTO var]
               | {APPEND | APPENDING} expr [INTO var]
               | {NCONC | NCONCING} expr [INTO var]
               | {COUNT | COUNTING} expr [INTO var]
               | {SUM | SUMMING} expr [INTO var]
               | {MAXIMIZE | MAXIMIZING} expr [INTO var]
               | {MINIMIZE | MINIMIZING} expr [INTO var]

;; Termination
termination ::= WHILE expr | UNTIL expr
              | ALWAYS expr | NEVER expr | THEREIS expr

;; Unconditional
unconditional ::= {DO | DOING} {expr}+
                | RETURN expr

;; Conditional
conditional ::= {IF | WHEN | UNLESS} expr clause {AND clause}* [ELSE clause {AND clause}*] [END]
clause      ::= accumulation | unconditional | conditional

;; Miscellaneous
initial-final ::= INITIALLY {expr}+ | FINALLY {expr}+
```

### 1.2 Clause Categories — Complete Reference

#### Iteration Drivers

| Driver | Syntax | Semantics |
|--------|--------|-----------|
| **Arithmetic up** | `for i from 0 to 10` | Counter, inclusive end |
| **Arithmetic up excl** | `for i from 0 below 10` | Counter, exclusive end |
| **Arithmetic down** | `for i from 10 downto 0` | Decrementing counter |
| **Arithmetic step** | `for i from 0 to 100 by 5` | Custom step size |
| **List elements** | `for x in xs` | `car` of each cons cell |
| **List tails** | `for x on xs` | Successive `cdr`s |
| **List custom step** | `for x in xs by #'cddr` | Custom step function |
| **Vector elements** | `for x across vec` | Array element iteration |
| **Computed** | `for x = init then step` | Initial value, then step expr |
| **Computed each** | `for x = expr` | Re-evaluated each iteration |
| **Hash keys** | `for k being the hash-keys of ht` | Hash table key iteration |
| **Hash values** | `for v being the hash-values of ht` | Hash table value iteration |
| **Hash key+val** | `for k being the hash-keys of ht using (hash-value v)` | Both simultaneously |

#### Accumulation Clauses

| Clause | Init | Step | Return |
|--------|------|------|--------|
| `collect expr` | `result = '()` | Append to tail (O(1) via tail pointer) | The accumulated list |
| `append expr` | `result = '()` | Splice list onto tail | The accumulated list |
| `nconc expr` | `result = '()` | Destructive splice | The accumulated list |
| `count expr` | `n = 0` | `(if expr (incf n))` | The count |
| `sum expr` | `s = 0` | `(incf s expr)` | The sum |
| `maximize expr` | `m = nil` | `(setf m (max m expr))` | The maximum |
| `minimize expr` | `m = nil` | `(setf m (min m expr))` | The minimum |

All accumulation clauses support `into var` to name the accumulator explicitly, allowing multiple independent accumulators in one loop.

#### Termination Clauses

| Clause | Semantics | Return value |
|--------|-----------|--------------|
| `while expr` | Stop when expr is nil | Accumulated value |
| `until expr` | Stop when expr is non-nil | Accumulated value |
| `always expr` | Stop returning nil if expr ever nil | `t` if never nil |
| `never expr` | Stop returning nil if expr ever non-nil | `t` if always nil |
| `thereis expr` | Stop returning value if expr non-nil | First non-nil value |
| `repeat n` | Stop after n iterations | Accumulated value |

#### Control Flow

| Clause | Semantics |
|--------|-----------|
| `do expr...` | Execute body expressions (side effects) |
| `return expr` | Immediately return value |
| `if/when expr clause [else clause] [end]` | Conditional execution |
| `unless expr clause [else clause] [end]` | Negated conditional |
| `initially expr...` | Execute in prologue (before loop) |
| `finally expr...` | Execute in epilogue (after loop) |
| `named name` | Names the block for `return-from` |

### 1.3 Key Semantic Details

**Parallel vs sequential stepping:** Multiple `for` clauses step in parallel. Each `for` clause's step expression is evaluated before any variable is updated. The `and` keyword chains `for` clauses to be explicitly parallel. Sequential stepping requires separate `for` clauses without `and`.

**Termination synchronization:** When multiple `for` clauses are present, the loop terminates when ANY driver is exhausted. The shortest sequence wins.

**The `it` pronoun:** In `(loop ... when (test x) collect it)`, `it` refers to the value of the `when` test expression. Avoids re-evaluation.

**Destructuring:** CL `loop` supports destructuring bind in `for`: `(loop for (a b . c) in list-of-lists ...)`.

**Default return:** If no accumulation clause is present, `loop` returns `nil`. If an accumulation clause is present without `into`, its value is the default return. Multiple accumulation clauses without `into` must all be compatible (all list-type or the same non-list type).

---

## 2. CL `loop` Internal Architecture

### 2.1 MIT `loop` (the canonical implementation)

The MIT `loop` implementation (~2000 lines, used by SBCL, CCL, ECL, Clasp with variations) is a **monolithic** macro that uses special variables for state accumulation during expansion:

**Phase 1: Parsing** — A recursive descent parser walks the flat token list. The parser recognizes keywords by symbol comparison (both symbols and keywords accepted: `for` and `:for` are equivalent). State is accumulated into special variables:

```
*loop-prologue*      — Forms for the prologue (variable inits, initially clauses)
*loop-body*          — The main loop body
*loop-epilogue*      — Forms for the epilogue (finally clauses)
*loop-before-loop*   — Setup before the tagbody
*loop-after-body*    — Stepping forms (parallel assignment)
*loop-after-epilogue* — Cleanup after epilogue
```

**Phase 2: Code generation** — Assembles the special variables into the expansion:

```lisp
;; Expansion skeleton:
(block LOOP-NAME
  (let (BINDINGS...)
    PROLOGUE...
    (tagbody
      LOOP-TAG
        BODY...
        AFTER-BODY...
        (go LOOP-TAG)
      END-TAG)
    EPILOGUE...))
```

**Key implementation details:**

- **`collect` uses a tail pointer**: Two hidden variables — `list-head` (a cons cell sentinel) and `list-tail` (pointer to last cell). Appending is O(1) via `(rplacd list-tail (cons val nil))` then advancing `list-tail`.
- **Multiple `for` clauses generate parallel stepping**: All step expressions are evaluated into temporaries, then assigned simultaneously (like Scheme `do`).
- **Termination tests are inserted at the top of the loop body** (before user code), so when a driver is exhausted, body and step forms are skipped.
- **`with` variables are bound in the outer `let`**, not stepped.
- **`initially` forms go after bindings but before the tagbody.**
- **`finally` forms go after the tagbody**, wrapped in the block so `return-from` can skip them.

### 2.2 SICL `loop` (modern modular implementation)

Robert Strandh's SICL `loop` (2016) takes a different approach using **combinator parsing** and **CLOS generic functions**:

**Phase 1: Combinator parsing** — Each clause type defines its own parser function. Parsers are combined with `alternative` and `sequence` combinators. This means clause parsers are **textually separate** — adding a new clause type means adding a new parser function and a new class, without modifying existing code.

**Phase 2: Clause representation** — Each parsed clause becomes a CLOS object (standard instance). A class hierarchy mirrors the spec: `variable-clause`, `main-clause`, `for-as-clause`, `accumulation-clause`, etc.

**Phase 3: Semantic analysis via generic functions:**
- `initial-bindings(clause)` → variables to bind in the outer `let`
- `final-bindings(clause)` → variables to bind in the inner `let`
- `termination-test(clause)` → test to insert at top of body
- `step-form(clause)` → stepping code for after-body
- `body-form(clause)` → code for the main body
- `accumulation-variables(clause)` → hidden accumulator variables

**Phase 4: Assembly** — Generic function results are collected and assembled into the same `block`/`let`/`tagbody` skeleton.

**Key insight:** The SICL approach is extensible — adding a new clause type (e.g., iterating over user-defined sequences) requires only defining a new class and methods, with no modification to existing code. This is the gold standard for maintainability.

### 2.3 The Generated Code Structure

Every CL `loop` expansion follows this skeleton:

```lisp
(block <name>                           ;; named, for return-from
  (let* (<with-vars>                    ;; with bindings
         <iter-vars>                    ;; for/as iteration variables
         <accum-vars>                   ;; hidden accumulator variables
         <internal-vars>)              ;; temporaries for parallel stepping
    <initially-forms>                  ;; initially clause body
    (tagbody
      <start-tag>
        <termination-tests>            ;; (when (endp list) (go end-tag))
        <body-forms>                   ;; user body (do, conditionals)
        <accumulation-forms>           ;; collect/sum/count updates
        <stepping-forms>               ;; parallel variable updates
        (go <start-tag>)               ;; loop back
      <end-tag>)
    <finally-forms>                    ;; finally clause body
    <default-return>))                ;; accumulated value or nil
```

---

## 3. The `iterate` Library — A Better `loop`

`iterate` (by Jonathan Amsterdam, 1990) is the primary alternative to CL `loop`. It addresses loop's main criticisms while being more powerful.

### 3.1 Key Differences from `loop`

| Aspect | CL `loop` | `iterate` |
|--------|----------|-----------|
| **Syntax** | Flat keyword stream (non-s-expression) | S-expression clauses within `(iter ...)` |
| **Clause ordering** | Variable clauses must precede main clauses | No ordering restriction |
| **Nesting** | `collect` etc. only at top level | `collect` can appear inside `if`, `case`, etc. |
| **Extensibility** | Not portably extensible | `defmacro-clause` and `defmacro-driver` |
| **Code walking** | No (just keyword parsing) | Yes (walks body looking for iterate clauses) |
| **Generators** | No | `generate`/`next` for lazy on-demand iteration |
| **Previous value** | Awkward via parallel `and` | `(for prev previous var)` |

### 3.2 `iterate` Architecture — Code Walker

Unlike `loop` which parses a flat keyword stream, `iterate` **walks the body tree** looking for known clause forms. This is why `collect` can appear inside arbitrary s-expressions:

```lisp
(iter (for x in '(1 2 3))
  (case x                          ;; ordinary Lisp case
    (1 (collect :a))               ;; iterate recognizes collect inside case!
    (2 (collect :b))))
```

The code walker:
1. Walks the body forms recursively
2. When it finds a known clause keyword (`for`, `collect`, `sum`, etc.) at the head of a form, it extracts it as an iterate clause
3. The clause is processed and its code is placed in the appropriate section (prologue, body, epilogue, stepping)
4. Non-clause forms pass through as regular body code

### 3.3 `iterate` Clause Extensibility Protocol

Users can define new clauses and drivers:

```lisp
;; Define a new accumulation clause
(defmacro-clause (MULTIPLY expr &optional INTO var)
  `(reducing ,expr by #'* initial-value 1 ,@(when var `(into ,var))))

;; Define a new iteration driver
(defmacro-driver (FOR var IN-CSV-FILE filename)
  (let ((stream (gensym)))
    `(progn
       (with ,stream = (open ,filename))
       (for ,var next (or (read-line ,stream nil)
                          (terminate))))))
```

### 3.4 `iterate`-Unique Features

**Generators (`generate`/`next`)** — Lazy iteration, advancing only on demand:
```lisp
(iter (for i in '(1 2 3 4 5))
      (generate c in-string "black")
      (if (oddp i) (next c))       ;; only advance c on odd i
      (format t "~a " c))
;; => b b l l a
```

**`finding ... maximizing/minimizing`** — Find the element that maximizes/minimizes an expression:
```lisp
(iter (for lst in '((a) (b c d) (e f)))
      (finding lst maximizing (length lst)))
;; => (B C D)
```

**`first-iteration-p`** — Boolean predicate for special first-iteration behavior.

**`reducing`** — Generalized reduction:
```lisp
(iter (for i in '(10 5 2))
      (reducing i by #'/ initial-value 100))
;; => 1
```

---

## 4. Cross-Dialect Comparison

### 4.1 Feature Matrix

| Feature | CL `loop` | CL `iterate` | Racket `for/*` | Clojure `for` | Sema (current) |
|---------|-----------|-------------|----------------|---------------|----------------|
| **Syntax** | Flat keywords | S-expression | S-expression | Vector + keywords | — |
| **List iteration** | `for x in` | `(for x in)` | `([x xs])` | `[x xs]` | `map`/named-let |
| **Numeric range** | `for i from/to/by` | `(for i from to by)` | `([i (in-range)])` | — | `(range)` + HOFs |
| **Hash iteration** | `being the hash-keys` | `(for (k v) in-hashtable)` | `([k (in-hash-keys)])` | — | — |
| **Collect** | `collect expr` | `(collect expr)` | `for/list` | Implicit (lazy) | `map` |
| **Sum** | `sum expr` | `(sum expr)` | `for/sum` | — | `foldl` |
| **Count** | `count expr` | `(count expr)` | — | — | `foldl` |
| **Max/Min** | `maximize`/`minimize` | `(maximize)`/`(minimize)` | — | — | `foldl` |
| **Filter** | `when expr collect` | `(when expr (collect))` | `#:when` | `:when` | `filter` |
| **Early exit** | `return`/`while`/`until` | `(leave)`/`(while)`/`(finish)` | `#:break` | `:while` | — |
| **Parallel iter** | Multiple `for` | Multiple `(for)` | Default | — | `zip` |
| **Nested (cartesian)** | — | — | `for*` | Default | — |
| **Generators** | — | `generate`/`next` | — | — | — |
| **Custom fold** | — | `reducing` | `for/fold` | — | `foldl` |
| **Extensible** | Not portably | `defmacro-clause`/`defmacro-driver` | Sequence protocol | — | — |
| **Destructuring** | `for (a b) in` | `(for (a b) in)` | — | Clojure destructuring | `let` destructuring |

### 4.2 What a Sema `loop` Would Replace

Current Sema patterns that would be replaced by a loop DSL:

```scheme
;; Pattern 1: Filter + map (very common)
;; Current:
(->> xs (filter (fn (x) (> x 0))) (map (fn (x) (* x x))))
;; With loop:
(loop for x in xs when (> x 0) collect (* x x))

;; Pattern 2: Accumulation with state
;; Current:
(foldl (fn (acc x) (if (> x 0) (+ acc x) acc)) 0 xs)
;; With loop:
(loop for x in xs when (> x 0) sum x)

;; Pattern 3: Find first match
;; Current:
(let loop ((items xs))
  (if (null? items) nil
    (if (predicate? (car items)) (car items)
      (loop (cdr items)))))
;; With loop:
(loop for x in xs thereis (and (predicate? x) x))

;; Pattern 4: Indexed iteration
;; Current:
(let loop ((items xs) (i 0) (acc '()))
  (if (null? items) (reverse acc)
    (loop (cdr items) (+ i 1) (cons (list i (car items)) acc))))
;; With loop:
(loop for x in xs for i from 0 collect (list i x))

;; Pattern 5: Iterate over hash map
;; Current:
(map (fn (pair) (list (car pair) (cdr pair))) (map/entries m))
;; With loop:
(loop for k v in-map m collect (list k v))

;; Pattern 6: Multiple accumulators
;; Current:
(let loop ((items xs) (sum 0) (count 0))
  (if (null? items) (list sum count)
    (loop (cdr items) (+ sum (car items)) (+ count 1))))
;; With loop:
(loop for x in xs sum x into total count x into n finally (list total n))
```

---

## 5. Sema Infrastructure for a Loop DSL

### 5.1 Available Building Blocks

| Building Block | Status | Notes |
|---|---|---|
| `defmacro` with rest params | ✅ | Can receive flat clause list via `(. clauses)` |
| Full evaluator at expansion time | ✅ | Macros can call `car`/`cdr`/`cons`/`append`/`symbol?`/`equal?` etc. |
| Helper function definitions | ✅ | `define` parser helpers before the `defmacro` |
| Recursive expansion | ✅ | Proven by `->`, `->>`, `as->`, `some->` |
| Auto-gensym `foo#` | ✅ | Hygienic temporaries |
| Quasiquote + splicing | ✅ | Full codegen power |
| VM compile-time expansion | ✅ | Macros expanded before bytecode compilation — zero runtime cost |
| Named `let` with TCO | ✅ | Primary desugaring target |
| `do` loop with parallel stepping | ✅ | Alternative desugaring target |
| Native function registration | ✅ | `NativeFn::simple()` / `NativeFn::with_ctx()` |
| `__vm-*` delegate pattern | ✅ | Precedent: `__vm-try-match`, `__vm-defmacro-form`, `__vm-destructure` |

### 5.2 The Hybrid Architecture (Recommended)

Based on analysis of the codebase, the best implementation strategy for Sema is a **hybrid** approach:

```
User writes:    (loop for x in xs when (> x 0) collect (* x x))
                         ↓
Thin defmacro:  (defmacro loop (. clauses) (loop/compile clauses))
                         ↓
Native Rust fn: loop/compile receives the unevaluated clause list
                as a Value (list of symbols and expressions),
                parses it in Rust with good error messages,
                and returns a desugared Value (Sema source AST)
                         ↓
Evaluator:      Evaluates the returned named-let / do expansion
                (tree-walker: direct execution, VM: lowered → compiled)
```

**Why this wins over pure-macro:** Rust parser gives excellent error messages, proper clause validation, and is easy to extend (one `match` arm per clause). The `__vm-*` delegate pattern already establishes this exact approach.

**Why this wins over special-form:** No dual-eval burden. No changes to `lower.rs` or `compiler.rs`. The expansion is standard Sema code that both backends already handle. Single implementation in one Rust file.

### 5.3 What Sema Does NOT Have (and doesn't need)

| CL Feature | Sema Equivalent | Impact on Loop |
|------------|-----------------|----------------|
| `tagbody`/`go` | Named `let` + TCO | Loop desugars to tail recursion instead of goto |
| `block`/`return-from` | `try`/`throw` | Early return uses throw-based escape |
| `rplacd` (destructive cons) | `set!` on accumulator list | Collect uses cons+reverse instead of tail pointer |
| `multiple-value-bind` | N/A | Not needed; use list returns |
| Type declarations | N/A | Skip — Sema is dynamically typed |
| CLOS generic dispatch | N/A | Not needed — Rust `match` is the dispatch |
| Package symbols | N/A | Skip — Sema has no packages |

---

## 6. Sema `loop` Design — Full Specification

### 6.1 Supported Clause Types

#### Iteration Drivers

```scheme
;; Arithmetic
(loop for i from 0 to 10 ...)
(loop for i from 0 below 10 ...)
(loop for i from 10 downto 0 ...)
(loop for i from 0 to 100 by 5 ...)

;; List iteration
(loop for x in xs ...)
(loop for x in xs by cddr ...)       ;; custom step (skip every other)

;; Map iteration
(loop for k v in-map m ...)           ;; Sema-specific: iterate hash map entries

;; Vector iteration
(loop for x across vec ...)

;; Computed values
(loop for x = (init) then (step x) ...)   ;; initial value + step function
(loop for x = (compute) ...)               ;; recomputed each iteration

;; Range (sugar)
(loop for i in (range 10) ...)             ;; already works via list iteration
```

#### Accumulation

```scheme
(loop ... collect expr)                ;; build list
(loop ... collect expr into var)       ;; named accumulator
(loop ... append expr)                 ;; splice lists
(loop ... sum expr)                    ;; running total
(loop ... count expr)                  ;; count truthy values
(loop ... maximize expr)               ;; track maximum
(loop ... minimize expr)               ;; track minimum
```

#### Filtering and Control Flow

```scheme
(loop ... when pred ...)               ;; conditional (apply following clause)
(loop ... unless pred ...)             ;; negated conditional
(loop ... do expr ...)                 ;; side effects
(loop ... return expr)                 ;; early return
```

#### Termination

```scheme
(loop ... while cond ...)              ;; stop when cond becomes nil
(loop ... until cond ...)              ;; stop when cond becomes non-nil
(loop ... always cond)                 ;; #t when cond is never nil
(loop ... never cond)                  ;; #t when cond is never non-nil
(loop ... thereis expr)                ;; return first truthy value
(loop repeat n ...)                    ;; fixed iteration count
```

#### Setup/Teardown

```scheme
(loop with x = init ...)              ;; local binding (not stepped)
(loop ... finally expr)                ;; executed after loop finishes
(loop ... initially expr)              ;; executed before loop starts
```

### 6.2 Destructuring

Use Sema's existing destructuring in `for ... in`:

```scheme
(loop for (a b) in '((1 2) (3 4) (5 6))
      collect (+ a b))
;; => (3 7 11)

(loop for {:keys (name age)} in records
      when (> age 18)
      collect name)
```

### 6.3 Multiple Accumulators

```scheme
(loop for x in xs
      sum x into total
      count #t into n
      finally (/ total n))   ;; compute average
```

### 6.4 Parallel Iteration

Multiple `for` clauses iterate in parallel (zip behavior), terminating when the shortest is exhausted:

```scheme
(loop for x in xs
      for i from 0
      collect (list i x))
;; => ((0 a) (1 b) (2 c))
```

### 6.5 What NOT to Support (Sema-specific omissions)

| CL Feature | Why Omit |
|------------|----------|
| `nconc` | Sema has no destructive list operations |
| `for x on xs` (tail iteration) | Rare; use `cdr` chain manually |
| `being the hash-keys of` | Replace with cleaner `for k v in-map m` |
| `being the symbols of` (package iteration) | Sema has no packages |
| Type declarations (`of-type fixnum`) | Sema is dynamically typed |
| `named name` | Use `try`/`throw` for early exit from named blocks |
| `loop-finish` | Use `return` |
| The `it` pronoun | Complexity; save for later |
| `and` for explicit parallel chaining | Implicit parallel is sufficient |

---

## 7. Implementation Architecture

### 7.1 Component Layout

```
crates/sema-stdlib/src/loop_compiler.rs    ← NEW: Clause parser + code generator (Rust)
crates/sema-stdlib/src/lib.rs              ← Registration of loop/compile native fn
crates/sema-eval/src/prelude.rs            ← 1-line defmacro wrapper
crates/sema/tests/dual_eval_test.rs        ← Dual-eval tests
```

### 7.2 Clause Parser — State Machine

The parser walks the flat clause list, consuming tokens and building an intermediate representation:

```rust
/// Parsed representation of a loop form.
struct LoopIR {
    name: Option<String>,              // (named ...)
    with_bindings: Vec<WithBinding>,   // (with var = expr)
    drivers: Vec<Driver>,              // (for var in/from/across ...)
    body_clauses: Vec<BodyClause>,     // do/collect/sum/when/unless/...
    initially: Vec<Value>,             // (initially ...)
    finally: Vec<Value>,               // (finally ...)
}

enum Driver {
    InList { var: Value, expr: Value, step_fn: Option<Value> },
    Across { var: Value, expr: Value },
    Arithmetic { var: Value, from: Option<Value>, to: Option<Value>,
                 to_kind: ToKind, by: Option<Value>, direction: Direction },
    Computed { var: Value, init: Value, then: Option<Value> },
    InMap { key_var: Value, val_var: Value, expr: Value },
}

enum BodyClause {
    Do(Vec<Value>),
    Collect { expr: Value, into: Option<Value> },
    Append { expr: Value, into: Option<Value> },
    Sum { expr: Value, into: Option<Value> },
    Count { expr: Value, into: Option<Value> },
    Maximize { expr: Value, into: Option<Value> },
    Minimize { expr: Value, into: Option<Value> },
    When { test: Value, clauses: Vec<BodyClause>, else_clauses: Vec<BodyClause> },
    Unless { test: Value, clauses: Vec<BodyClause> },
    While(Value),
    Until(Value),
    Always(Value),
    Never(Value),
    Thereis(Value),
    Return(Value),
    Repeat(Value),
}
```

### 7.3 Parser Token Consumption

```rust
fn parse_loop(tokens: &[Value]) -> Result<LoopIR, SemaError> {
    let mut ir = LoopIR::new();
    let mut pos = 0;

    while pos < tokens.len() {
        let sym = tokens[pos].as_symbol()
            .ok_or_else(|| SemaError::eval(
                format!("loop: expected clause keyword, got {}", tokens[pos]))
                .with_hint("valid clause keywords: for, with, do, collect, sum, when, while, ..."))?;

        match sym.as_str() {
            "for" | "as" => {
                let (driver, consumed) = parse_for_clause(&tokens[pos+1..])?;
                ir.drivers.push(driver);
                pos += 1 + consumed;
            }
            "with" => {
                let (binding, consumed) = parse_with(&tokens[pos+1..])?;
                ir.with_bindings.push(binding);
                pos += 1 + consumed;
            }
            "collect" | "collecting" => {
                let (clause, consumed) = parse_accumulation("collect", &tokens[pos+1..])?;
                ir.body_clauses.push(clause);
                pos += 1 + consumed;
            }
            // ... remaining clauses ...
            other => {
                return Err(SemaError::eval(format!("loop: unknown clause '{other}'"))
                    .with_hint("valid clauses: for, with, do, collect, sum, count, \
                               maximize, minimize, when, unless, while, until, \
                               always, never, thereis, repeat, return, initially, finally"));
            }
        }
    }
    Ok(ir)
}
```

### 7.4 Code Generator

The `LoopIR` produces a Sema `Value` AST. The expansion strategy depends on what clauses are present:

```rust
impl LoopIR {
    fn emit(&self) -> Result<Value, SemaError> {
        // 1. Determine accumulator variables and their init/finalize forms
        let accumulators = self.analyze_accumulators()?;

        // 2. Determine the default return value
        let return_value = self.default_return(&accumulators);

        // 3. Build the driver stepping code
        let (driver_bindings, driver_tests, driver_steps) = self.compile_drivers()?;

        // 4. Build the body
        let body = self.compile_body(&accumulators)?;

        // 5. Assemble into named-let or do loop
        self.assemble(driver_bindings, driver_tests, driver_steps, body,
                      &accumulators, return_value)
    }
}
```

---

## 8. Desugaring Rules

### 8.1 Simple collect

```scheme
;; Source:
(loop for x in xs collect (* x x))

;; Desugars to:
(let __loop ((items# xs) (acc# '()))
  (if (null? items#)
    (reverse acc#)
    (let ((x (car items#)))
      (__loop (cdr items#) (cons (* x x) acc#)))))
```

### 8.2 Filter + collect

```scheme
;; Source:
(loop for x in xs when (> x 0) collect (* x x))

;; Desugars to:
(let __loop ((items# xs) (acc# '()))
  (if (null? items#)
    (reverse acc#)
    (let ((x (car items#)))
      (if (> x 0)
        (__loop (cdr items#) (cons (* x x) acc#))
        (__loop (cdr items#) acc#)))))
```

### 8.3 Arithmetic range

```scheme
;; Source:
(loop for i from 0 below 10 by 2 collect i)

;; Desugars to:
(let __loop ((i 0) (acc# '()))
  (if (>= i 10)
    (reverse acc#)
    (__loop (+ i 2) (cons i acc#))))
```

### 8.4 Parallel iteration (zip)

```scheme
;; Source:
(loop for x in xs for i from 0 collect (list i x))

;; Desugars to:
(let __loop ((items# xs) (i 0) (acc# '()))
  (if (null? items#)
    (reverse acc#)
    (let ((x (car items#)))
      (__loop (cdr items#) (+ i 1) (cons (list i x) acc#)))))
```

### 8.5 Sum accumulation

```scheme
;; Source:
(loop for x in xs sum x)

;; Desugars to:
(let __loop ((items# xs) (sum# 0))
  (if (null? items#)
    sum#
    (let ((x (car items#)))
      (__loop (cdr items#) (+ sum# x)))))
```

### 8.6 Multiple accumulators + finally

```scheme
;; Source:
(loop for x in xs
      sum x into total
      count #t into n
      finally (/ total n))

;; Desugars to:
(let __loop ((items# xs) (total 0) (n 0))
  (if (null? items#)
    (begin (/ total n))     ;; finally clause
    (let ((x (car items#)))
      (__loop (cdr items#) (+ total x) (+ n 1)))))
```

### 8.7 Early return with thereis

```scheme
;; Source:
(loop for x in xs thereis (and (> x 10) x))

;; Desugars to:
(let __loop ((items# xs))
  (if (null? items#)
    #f
    (let ((x (car items#)))
      (let ((result# (and (> x 10) x)))
        (if result# result#
          (__loop (cdr items#)))))))
```

### 8.8 Map iteration (Sema-specific)

```scheme
;; Source:
(loop for k v in-map m collect (list k v))

;; Desugars to:
(let __loop ((entries# (map/entries m)) (acc# '()))
  (if (null? entries#)
    (reverse acc#)
    (let ((k (car (car entries#)))
          (v (cdr (car entries#))))
      (__loop (cdr entries#) (cons (list k v) acc#)))))
```

### 8.9 with + while + do (imperative)

```scheme
;; Source:
(loop with i = 0
      while (< i 10)
      do (println i) (set! i (+ i 1)))

;; Desugars to:
(let ((i 0))
  (let __loop ()
    (if (not (< i 10))
      nil
      (begin
        (println i)
        (set! i (+ i 1))
        (__loop)))))
```

### 8.10 Destructuring

```scheme
;; Source:
(loop for (a b) in '((1 2) (3 4)) collect (+ a b))

;; Desugars to:
(let __loop ((items# '((1 2) (3 4))) (acc# '()))
  (if (null? items#)
    (reverse acc#)
    (let ((__pair# (car items#)))
      (let ((a (car __pair#))
            (b (cadr __pair#)))
        (__loop (cdr items#) (cons (+ a b) acc#))))))
```

---

## 9. Error Handling Strategy

### 9.1 Parse-Time Errors (in Rust)

Because the clause parser is a native Rust function, all parse errors get full `.with_hint()` support:

```rust
// Unknown clause
SemaError::eval("loop: unknown clause 'ford'")
    .with_hint("did you mean 'for'? Valid clauses: for, with, do, collect, ...")

// Missing expression after keyword
SemaError::eval("loop: 'collect' requires an expression")
    .with_hint("usage: (loop ... collect EXPR [into VAR])")

// Invalid for-clause driver
SemaError::eval("loop: 'for x' missing driver — expected 'in', 'from', 'across', '=', or 'in-map'")

// Type mismatch in arithmetic
SemaError::eval("loop: 'from' value must be followed by 'to', 'below', 'above', 'downto', or 'by'")

// Conflicting accumulators
SemaError::eval("loop: cannot use both 'collect' and 'sum' without 'into' — \
                 they would accumulate into the same unnamed variable")
    .with_hint("use 'collect EXPR into VAR' and 'sum EXPR into VAR' to separate them")
```

### 9.2 Runtime Errors

The generated named-let code will produce standard Sema runtime errors:
- `car: expected pair, got X` — when iterating over a non-list
- Standard type errors from body expressions

### 9.3 macroexpand Support

```scheme
(macroexpand '(loop for x in xs when (> x 0) collect (* x x)))
;; Shows the generated named-let form — fully transparent
```

---

## 10. Testing Strategy

### 10.1 Dual-Eval Tests

Every loop feature must pass in both tree-walker and VM backends:

```rust
dual_eval_tests! {
    // Basic iteration
    loop_collect_basic: "(loop for x in '(1 2 3) collect (* x x))" => "(1 4 9)",
    loop_sum_basic: "(loop for x in '(1 2 3) sum x)" => "6",
    loop_count_basic: "(loop for x in '(1 2 3 4) count (> x 2))" => "2",

    // Arithmetic ranges
    loop_from_to: "(loop for i from 0 to 4 collect i)" => "(0 1 2 3 4)",
    loop_from_below: "(loop for i from 0 below 4 collect i)" => "(0 1 2 3)",
    loop_from_downto: "(loop for i from 3 downto 0 collect i)" => "(3 2 1 0)",
    loop_from_by: "(loop for i from 0 to 10 by 3 collect i)" => "(0 3 6 9)",

    // Filtering
    loop_when_collect: "(loop for x in '(1 -2 3 -4 5) when (> x 0) collect x)" => "(1 3 5)",
    loop_unless_collect: "(loop for x in '(1 -2 3 -4 5) unless (> x 0) collect x)" => "(-2 -4)",

    // Termination
    loop_while: "(loop for x in '(1 2 3 0 4 5) while (> x 0) collect x)" => "(1 2 3)",
    loop_until: "(loop for x in '(1 2 3 0 4 5) until (= x 0) collect x)" => "(1 2 3)",
    loop_thereis: "(loop for x in '(1 3 5 6 7) thereis (and (even? x) x))" => "6",
    loop_always: "(loop for x in '(2 4 6) always (even? x))" => "#t",
    loop_never: "(loop for x in '(2 4 6) never (odd? x))" => "#t",
    loop_repeat: "(loop repeat 3 collect 'x)" => "(x x x)",

    // Parallel iteration
    loop_parallel: "(loop for x in '(a b c) for i from 0 collect (list i x))" => "((0 a) (1 b) (2 c))",

    // Multiple accumulators
    loop_multi_accum: "(loop for x in '(1 2 3) sum x into s count #t into n finally (list s n))"
                      => "(6 3)",

    // with binding
    loop_with: "(loop with base = 10 for x in '(1 2 3) collect (+ x base))" => "(11 12 13)",

    // Computed variable
    loop_computed: "(loop for x = 1 then (* x 2) repeat 5 collect x)" => "(1 2 4 8 16)",

    // do (side effects + return nil)
    loop_do_return: "(begin (define r '()) (loop for x in '(1 2 3) do (set! r (cons x r))) r)"
                    => "(3 2 1)",

    // Destructuring
    loop_destructure: "(loop for (a b) in '((1 2) (3 4)) collect (+ a b))" => "(3 7)",

    // Maximize/minimize
    loop_maximize: "(loop for x in '(3 1 4 1 5 9) maximize x)" => "9",
    loop_minimize: "(loop for x in '(3 1 4 1 5 9) minimize x)" => "1",

    // across (vectors)
    loop_across: "(loop for x across [1 2 3] collect (* x 10))" => "(10 20 30)",

    // Nested conditionals
    loop_when_else: "(loop for x in '(1 2 3 4) when (even? x) collect x else collect (- x))"
                    => "(-1 2 -3 4)",

    // Return
    loop_return: "(loop for x in '(1 2 3 4 5) when (> x 3) return x)" => "4",
}
```

### 10.2 Error Tests

```rust
dual_eval_error_tests! {
    loop_unknown_clause: "(loop ford x in xs)" => "unknown clause",
    loop_missing_in: "(loop for x xs)" => "expected 'in'",
    loop_collect_no_expr: "(loop for x in '(1 2 3) collect)" => "requires an expression",
    loop_conflicting_accum: "(loop for x in xs collect x sum x)" => "cannot use both",
}
```

### 10.3 Serialization Round-Trip

Add to `serialize_roundtrip_test.rs` to verify loop works through compile → serialize → deserialize → execute:

```rust
#[test]
fn roundtrip_loop_collect() {
    assert_roundtrip_eq(
        "(loop for x in '(1 2 3) collect (* x x))",
        Value::list(vec![Value::int(1), Value::int(4), Value::int(9)]),
    );
}
```

---

## Summary

| Aspect | Decision |
|--------|----------|
| **Scope** | Full CL-style loop DSL with ~25 clause types |
| **Syntax** | Flat keyword stream: `(loop for x in xs when (> x 0) collect (* x x))` |
| **Architecture** | Hybrid: 1-line `defmacro` wrapper + native Rust `loop/compile` function |
| **Location** | `crates/sema-stdlib/src/loop_compiler.rs` (~400-600 lines Rust) |
| **Desugaring target** | Named `let` (TCO-guaranteed in both backends) |
| **VM changes** | None (macro expands before compilation) |
| **Error handling** | Rust parser with `.with_hint()` — good messages |
| **Sema-specific additions** | `for k v in-map m` (hash map iteration), `across` for vectors |
| **Omissions vs CL** | `nconc`, `on`, package iteration, type declarations, `it`, `named` |
| **Estimated effort** | ~500 lines Rust + ~50 lines tests |
