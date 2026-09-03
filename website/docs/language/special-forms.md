---
outline: [2, 3]
---

# Special Forms

Special forms are recognized by the evaluator when their name appears first in
a list. Unlike function calls, they can choose which operands to evaluate,
introduce bindings, or change control flow.

Special-form names can be bound and used as values. They cannot currently be
overridden in operator position: in `(if ...)`, `if` is always the special form,
even if a local binding has the same name. See
[Special-form names in operator position](https://github.com/sema-lisp/sema/blob/main/docs/limitations.md#36-special-form-names-win-over-local-bindings-in-operator-position).

Only `nil` and `#f` are falsy. All other values, including `0`, `""`, and empty
collections, are truthy.

This page also covers closely related syntax and prelude macros. The label on
each group states which constructs are evaluator special forms and which are
not.

## Complete Index

| Category | Names |
|---|---|
| Definitions and functions | [`define`](#define) (`def`), [`defun`](#defun) (`defn`), [`set!`](#set), [`lambda`](#lambda) (`fn`) |
| Conditionals and logic | [`if`](#if), [`cond`](#cond), [`case`](#case), [`when`](#when), [`unless`](#unless), [`and`](#and), [`or`](#or) |
| Bindings and sequencing | [`let`](#let), [`let*`](#let-1), [`letrec`](#letrec), [`begin`](#begin) (`progn`), [`do`](#do), [`while`](#while) |
| Quoting and macros | [`quote`](#quote), [`quasiquote`](#quasiquote), [`defmacro`](./macros-modules.md#defmacro), [`define-syntax`](./macros-modules.md#define-syntax-syntax-rules), [`macroexpand`](./macros-modules.md#macroexpand) |
| Data dispatch | [`match`](#match), [`match*`](#match-lenient-variant), [`define-record-type`](#define-record-type), [`defmulti`](#defmulti), [`defmethod`](#defmethod) |
| Multiple values | [`let-values`](#let-values), [`let*-values`](#let-values-1), [`define-values`](#define-values) |
| Errors and evaluation | [`throw`](#throw), [`try`](#try-catch), [`eval`](#eval) |
| Files and modules | [`load`](#load), [`module`](./macros-modules.md#module), [`export`](./macros-modules.md#module), [`import`](./macros-modules.md#import) |
| Lazy and async evaluation | [`delay`](#delay), [`force`](#force), [`async`](#async), [`await`](#await) |
| LLM values | [`prompt`](../llm/prompts.md#prompt), [`message`](../llm/prompts.md#message), [`deftool`](../llm/tools-agents.md#deftool), [`defagent`](../llm/tools-agents.md#defagent) |

Aliases in parentheses are separate evaluator-recognized names with identical
behavior. Threading, conditional-binding, `guard`, `parameterize`, `dotimes`,
and `for-range` are prelude macros. `#(...)` is reader syntax. `values`,
`call-with-values`, `make-parameter`, `raise`, and the promise predicates are
ordinary callable values.

## Definitions & Assignment

### `define`

Evaluate a value and bind it in the current environment. Given a signature list
rather than a bare symbol, `define` creates a named function whose body runs in
the lexical environment of the definition. A vector or map in binding position
destructures the value. `define` returns `nil`.

```sema
(define x 42)                          ; bind a value
(define (square x) (* x x))            ; same as (define square (fn (x) (* x x)))
```

::: tip Clojure alias
`def` is accepted as an alias for `define`.
:::

### `set!`

Evaluate `value`, then replace an existing lexical or global binding. It is an
error if the name is unbound. `set!` returns `nil`.

```sema
(set! x 99)
```

## Quoting

### `quote`

Return the argument without evaluating it. The reader shorthand `'x` desugars to `(quote x)`.

```sema
(quote (+ 1 2))                        ; => (+ 1 2) ; list data
'(+ 1 2)                               ; same thing
'foo                                   ; => foo (the symbol, not its value)
```

### `quasiquote`

Template with selective evaluation. Use `` ` `` as shorthand. Inside a
quasiquote, `,expr` (unquote) evaluates and inserts one value, while `,@expr`
(unquote-splicing) evaluates a list or vector and inserts each of its elements.

```sema
(define x 42)
`(a b ,x)                              ; => (a b 42)
`(a ,@(list 1 2 3) b)                  ; => (a 1 2 3 b)
```

Quasiquote is essential for writing macros — see [Macros](./macros-modules.md#macros).

## Functions

### `lambda`

Create an anonymous function that closes over its lexical environment. The
parameter specification can be a list or vector. Dotted list notation captures
rest arguments. The body is evaluated in order and returns its last value.

```sema
(lambda (x y) (+ x y))
```

### `fn`

Alias for `lambda`.

```sema
(fn (x) (* x x))
(fn (x . rest) rest)                   ; rest parameters with dot notation
```

### `defun`

Define a named function, equivalent to `(define (name params...) body...)`.
Parameters must be symbols, with optional dotted rest notation. The body is
evaluated in order and its last expression is in tail position. `defun` returns
`nil`.

```sema
(defun square (x) (* x x))
(defun greet (name) f"Hello, ${name}!")
```

::: tip Clojure alias
`defn` is accepted as an alias for `defun`.
:::

## Conditionals

### `if`

Evaluate `condition`, then evaluate and return exactly one branch. Only `nil`
and `#f` are falsy. Every other value, including `0`, `""`, and the empty list
`()`, is truthy. `nil` and `()` are distinct values.

The else expression is optional. If it is absent and the condition is falsy,
`if` returns `nil`:

```sema
(if (> x 0) "positive" "non-positive")
(if nil "yes" "no")                    ; => "no"
(if #f "unreachable")                  ; => nil
(if '() "yes" "no")                    ; => "yes"
(if #t (println "selected") (error "not evaluated"))
```

### `cond`

Evaluate clause tests from left to right and evaluate only the first selected
body. A test-only clause returns `#t` when selected. If no clause matches,
return `nil`. An `else` clause is a catch-all; put it last because later clauses
are ignored.

```sema
(cond
  ((< x 0) "negative")
  ((= x 0) "zero")
  (else "positive"))
```

### `case`

Evaluate the discriminant once, then compare it with each clause's quoted
datums. Evaluate only the first matching body. Return `nil` if nothing matches
and there is no `else` clause.

```sema
(case (:status response)
  ((:ok) "success")
  ((:error :timeout) "failure")
  (else "unknown"))
```

### `when`

Evaluate the body in order only when the condition is truthy, and return the
last body value. Return `nil` when the condition is falsy.

```sema
(when (> x 0) (println "positive"))
```

### `unless`

Evaluate the body in order only when the condition is falsy, and return the
last body value. Return `nil` when the condition is truthy.

```sema
(unless (> x 0) (println "not positive"))
```

## Threading Macros

Prelude macros for pipeline-style code. They expand before evaluation and are
available automatically; no import is needed.

### `->`

Thread-first: inserts the value as the first argument of each form.

```sema
(-> 5 (+ 3) (* 2))                    ; => 16
(-> response :body json/decode :data)  ; nested access
```

### `->>`

Thread-last: inserts the value as the last argument of each form.

```sema
(->> (range 1 100)
     (filter even?)
     (map #(* % %))
     (take 5))                         ; => (4 16 36 64 100)
```

### `as->`

Thread-as: bind the threaded value to a name for arbitrary placement.

```sema
(as-> 5 x (+ x 3) (* x x) (- x 1))   ; => 63
```

### `some->`

Nil-safe thread-first: stops and returns `nil` if any step produces `nil`.

```sema
(some-> config :database :connection-string db/connect)
;; returns nil if any step is nil, instead of crashing
```

## Conditional Binding

These are prelude macros, not evaluator special forms.

### `when-let`

Bind a value and execute body only if non-nil.

```sema
(when-let (user (db/find-user id))
  (send-email user "Welcome back"))
```

### `if-let`

Bind a value and branch on nil/non-nil.

```sema
(if-let (cached (cache/get key))
  cached
  (compute-fresh-value))
```

Both forms take exactly one binding, written `(name value)` or `[name value]` —
not the doubled `((name value))` of `let`. The doubled form is reported as a
`let` binding error.

## Short Lambda

### `#(...)`

Reader syntax for concise anonymous functions. `%` (or `%1`) is the first
argument, `%2` the second, and so on. The reader expands this syntax to `fn`.

```sema
(map #(+ % 1) '(1 2 3))               ; => (2 3 4)
(map #(* % %) '(1 2 3 4))             ; => (1 4 9 16)
(filter #(> % 3) '(1 2 3 4 5))        ; => (4 5)
(#(+ %1 %2) 3 4)                      ; => 7
```

## Bindings

### `let`

Parallel bindings — all init expressions are evaluated before any binding is created.

```sema
(let ((x 10) (y 20))
  (+ x y))
```

Each binding is its own list: `((x 10) (y 20))`. The flat Clojure spelling
`(let [x 10 y 20] ...)` is not accepted.

### `let*`

Sequential bindings — each binding is visible to subsequent ones.

```sema
(let* ((x 10) (y (* x 2)))
  (+ x y))
```

### `letrec`

Recursive bindings — all bindings are visible to all init expressions. Useful for mutually recursive functions.

```sema
(letrec ((even? (fn (n) (if (= n 0) #t (odd? (- n 1)))))
         (odd?  (fn (n) (if (= n 0) #f (even? (- n 1))))))
  (even? 10))
```

### Named `let`

Loop construct with tail-call optimization.

```sema
(let loop ((i 0) (sum 0))
  (if (= i 100)
    sum
    (loop (+ i 1) (+ sum i))))
```

## Destructuring

`let`, `let*`, `letrec`, `define`, and `lambda` all support destructuring
patterns in binding positions. In a function, a pattern occupies one parameter
position, so it must appear inside the parameter list.

### Vector Destructuring

Extract elements from lists and vectors by position.

```sema
(let (([a b c] '(1 2 3)))
  (+ a b c))                           ; => 6

(let (([first & rest] '(1 2 3 4)))
  rest)                                 ; => (2 3 4)

(let (([_ second] '(1 2)))
  second)                               ; => 2
```

### Map Destructuring

Extract values from maps using `{:keys [...]}`.

```sema
(let (({:keys [name age]} {:name "Alice" :age 30}))
  (println name))                       ; prints "Alice"
```

Explicit key-pattern pairs:

```sema
(let (({:x val} {:x 42}))
  val)                                  ; => 42
```

Missing map keys bind `nil`. Vector patterns are strict: without `& rest`, the
input must have exactly the same number of elements as the pattern.

### Destructuring in `define`

```sema
(define [a b c] '(1 2 3))              ; binds a=1, b=2, c=3
(define {:keys [host port]} config)     ; binds host, port from map
```

### Destructuring in Function Parameters

```sema
(define sum-pair
  (fn ([a b]) (+ a b)))
(sum-pair '(3 4))                       ; => 7

(define greet
  (fn ({:keys [name title]})
    (format "Hello ~a ~a" title name)))
(greet {:name "Smith" :title "Dr."})    ; => "Hello Dr. Smith"
```

The function-signature shorthand `(define (name params ...) body ...)` and
`defun`/`defn` currently require plain symbol parameters. Use `fn` or `lambda`
as above when a parameter needs destructuring.

Nested patterns are supported:

```sema
(let (([[a b] c] '((1 2) 3)))
  (+ a b c))                           ; => 6
```

## Pattern Matching

### `match`

Match a value against patterns with optional guards.

```sema
(match value
  (pattern body ...)
  (pattern when guard body ...)
  ...)
```

If no clause matches, `match` **raises an error** (`match: no clause matched value: …`) — a non-exhaustive match is almost always a bug, so it fails loudly rather than returning `nil` silently. Add a catch-all `(_ ...)` clause to handle the rest:

```sema
(match status
  (:ok "success")
  (_   "other"))          ; catch-all; without it, an unmatched status raises
```

#### `match*` — lenient variant

When "no match" is a normal outcome (e.g. a lookup), use `match*`, which returns `nil` instead of raising:

```sema
(match* 42
  (1 "one")
  (2 "two"))              ; => nil  (no clause matched)
```

#### Literal Matching

```sema
(match status
  (:ok "success")
  (:error "failure")
  (_ "unknown"))
```

#### Binding Patterns

Symbols bind the matched value. `_` is a wildcard.

```sema
(match (+ 1 2)
  (x (format "got ~a" x)))             ; => "got 3"
```

#### Vector Patterns

```sema
(match '(1 2 3)
  ([a b c] (+ a b c)))                 ; => 6

(match args
  ([] (print-help))
  ([cmd & rest] (dispatch cmd rest)))
```

#### Map Patterns

Explicit key-pattern pairs are structural: each explicit key must exist in the
value or the clause does not match.

```sema
(match response
  ({:type :ok :data d}   (process d))
  ({:type :error :msg m} (log-error m))
  (_                     (println "unknown")))
```

With `{:keys [...]}` shorthand:

```sema
(match config
  ({:keys [host port]} (connect host port)))
```

Unlike an explicit key-pattern pair, `:keys` extraction does not require a key
to exist. A missing key binds its symbol to `nil`.

#### Guards

Add `when` after a pattern for conditional matching:

```sema
(match n
  (x when (> x 100) "big")
  (x when (> x 0)   "small")
  (_                 "non-positive"))
```

#### Nested Patterns

```sema
(match '(1 (2 3))
  ([a [b c]] (+ a b c)))               ; => 6
```

## Multiple Values

R7RS multiple return values: a producer can return more than one value from a single expression — without packing them into a list — and a small family of forms spreads or binds those values.

### `values`

Produce zero or more values, for consumption by `call-with-values`, `let-values`, `let*-values`, or `define-values`.

`(values x)` — exactly one value — is identity: it returns `x` unchanged, so a single-value `values` call flows through ordinary contexts as if `values` weren't there.

```sema
(+ (values 5) 1)                       ; => 6 (one value is identity)
(let-values (((a b) (values 1 2)))
  (+ a b))                             ; => 3
```

::: warning Escaping bundles
Zero or two-or-more values only make sense when consumed by one of the values-consuming forms. Letting a bundle escape into an ordinary single-value context — `(list (values 1 2))`, printing it, storing it — is unspecified by R7RS; Sema currently represents it as an opaque record (`#<record %multiple-values% 1 2>`) rather than silently spreading it into arguments.
:::

### `call-with-values`

The lower-level primitive: call a zero-argument `producer` thunk, then apply `consumer` to whatever it produced. A `values` bundle becomes separate arguments; an ordinary single value is passed as the consumer's one argument.

```sema
(call-with-values (lambda () (values 1 2)) +)           ; => 3
(call-with-values (lambda () (values 1 2 3)) list)      ; => (1 2 3)
(call-with-values (lambda () 42) list)                  ; => (42) ; single value, not spread
(call-with-values (lambda () (values)) (lambda () 99))  ; => 99   ; zero values
```

If the number of produced values doesn't match the consumer's arity, the call fails with the ordinary arity error (R7RS's "wrong number of values"). Note that producer/consumer are invoked through the same native dispatch as `apply`, so a call across this boundary is not a true VM tail call — deep recursion written through `call-with-values` won't get the same tail-call optimization as a plain named `let`.

### `let-values`

Bind the values produced by one or more producers to local names. Like `let`, binding is **parallel**: every producer is evaluated against the outer environment before any clause's names come into scope.

Each clause's formals can be `(a b)` (exact count), dotted `(a . rest)` (fixed prefix, remaining values as a list), or a bare symbol (all values as a list):

```sema
(let-values (((a b) (values 1 2)))
  (+ a b))                             ; => 3

(let-values (((a . rest) (values 1 2 3)))
  rest)                                ; => (2 3)

(let-values ((all (values 1 2 3)))
  all)                                 ; => (1 2 3)
```

```sema
(define a 100)
(let-values (((a) (values 1))
             ((b) (values a)))         ; sees the OUTER a (100)
  b)                                   ; => 100
```

### `let*-values`

Like `let-values`, but binding is **sequential** — each producer sees every earlier clause's bindings:

```sema
(define a 100)
(let*-values (((a) (values 1))
              ((b) (values a)))        ; sees the NEW a from the clause above
  b)                                   ; => 1
```

### `define-values`

The `define` analogue: bind produced values as top-level (or body-local) definitions. Formals follow the same rules as `let-values`.

```sema
(define-values (a b) (values 10 20))
(+ a b)                                ; => 30

(define-values (q . r) (values 1 2 3))
r                                      ; => (2 3)
```

## Dynamic Binding

R7RS parameter objects: values that can be rebound for the dynamic extent of a body and automatically restored on exit.

### `make-parameter`

Create a **parameter** — a zero-argument procedure that returns its current value. An optional converter is applied to the initial value immediately and to every value later installed (once per install, never on restore).

```sema
(define radix (make-parameter 10))
(radix)                                ; => 10

(define scale (make-parameter 1 (lambda (x) (* x 2))))
(scale)                                ; => 2 (converter already applied to init)
```

Calling a parameter with one argument mutates it directly (SRFI-39 style) — but for scoped rebinding, prefer `parameterize`.

### `parameterize`

`parameterize` is a prelude macro that uses parameter builtins and
`try`/`catch` to restore prior values.

Evaluate each value, convert it through the parameter's converter, install the converted values, run `body`, and always restore every parameter to its **prior** value before returning — even if `body` raises (the condition is re-raised after restoration).

```sema
(define radix (make-parameter 10))

(parameterize ((radix 16))
  (radix))                             ; => 16

(radix)                                ; => 10 (restored)
```

Restoration also happens on a non-local exit via `raise`:

```sema
(define mode (make-parameter :normal))

(guard (e (else (mode)))
  (parameterize ((mode :debug))
    (raise "boom")))                   ; => :normal ; restored before the guard ran
```

`parameterize` forms nest — an inner form restores back to the outer one's value, not the original:

```sema
(parameterize ((mode :outer))
  (list (mode)
        (parameterize ((mode :inner)) (mode))
        (mode)))                       ; => (:outer :inner :outer)
```

Conversion happens once, at install time; restoration puts the saved value back raw, so a non-idempotent converter can't drift the parameter across repeated entries.

::: warning Async tasks
Restoration is unwind-on-error only (Sema has no `call/cc`). If a `parameterize` body suspends at an async yield point instead of returning or raising, the parameter stays bound across the yield and can be observed by sibling tasks until the body resumes. Synchronous dynamic scoping is fully correct.
:::

## Sequencing & Logic

### `begin`

Evaluate expressions in order and return the last result. An empty `begin`
returns `nil`.

```sema
(begin expr1 expr2 ... exprN)
```

::: tip Common Lisp alias
`progn` is accepted as an alias for `begin`.
:::

### `and`

Evaluate expressions from left to right. Return the first falsy value, or the
last value if all are truthy. With no arguments, return `#t`.

```sema
(and a b c)
(and #t 42)                            ; => 42
(and #t nil)                           ; => nil
(and)                                  ; => #t
```

### `or`

Evaluate expressions from left to right. Return the first truthy value, or the
last falsy value if none are truthy. With no arguments, return `#f`.

```sema
(or a b c)
(or nil 42)                            ; => 42
(or nil)                               ; => nil
(or)                                   ; => #f
```

## Iteration

`while` and `do` are evaluator special forms. `dotimes` and `for-range` are
prelude macros that expand to `do`.

### `while`

Evaluate the condition before each iteration and run the body while it remains
truthy. Return `nil`. Use `set!` to mutate loop state.

```sema
(let ((n 0))
  (while (< n 3)
    (println n)
    (set! n (+ n 1)))
  n)
;; prints 0, 1, 2
;; => 3
```

### `do`

Scheme `do` loop with variable bindings, step expressions, and a termination
test. Initial values are evaluated before the loop. After each body execution,
the step expressions compute the next bindings in parallel. When the test is
truthy, return the last result expression, or `nil` if none is present.

```sema
;; (do ((var init step) ...) (test result ...) body ...)
(do ((i 0 (+ i 1))
     (sum 0 (+ sum i)))
    ((= i 10) sum))                    ; => 45
```

With a body for side effects:

```sema
(do ((i 0 (+ i 1)))
    ((= i 5))
  (println i))                         ; prints 0..4
```

### `dotimes`

Evaluate a body `count` times with `var` bound to integers from `0` through
`count - 1`. Return `nil`. A zero or negative count skips the body.

```sema
(dotimes (i 3)
  (println i))                         ; prints 0, 1, 2
```

### `for-range`

Iterate from an inclusive start to an exclusive end. The optional positive
step defaults to `1`. Backward iteration is not supported.

```sema
(for-range (i 0 6 2)
  (println i))                         ; prints 0, 2, 4
```

## Lazy Evaluation

### `delay`

Create a lazy promise without evaluating the expression. The promise captures
the current lexical environment.

```sema
(define p (delay (+ 1 2)))
```

### `force`

Evaluate a lazy promise at most once and memoize its result. Later calls return
the stored value. Non-promise values pass through unchanged.

```sema
(force p)                              ; => 3 (evaluate and memoize)
(force p)                              ; => 3 (returns cached value)
(force 42)                             ; => 42 (non-promise passes through)
```

### `promise?`

Check if a value is a promise.

```sema
(promise? p)                           ; => #t
```

### `promise-forced?`

Check if a promise has already been forced.

```sema
(promise-forced? p)                    ; => #t (after forcing)
```

## Record Types

### `define-record-type`

Define a record type with constructor, predicate, and field accessors.

```sema
(define-record-type point
  (make-point x y)
  point?
  (x point-x)
  (y point-y))

(define p (make-point 3 4))
(point? p)                             ; => #t
(point-x p)                           ; => 3
(point-y p)                           ; => 4
(record? p)                           ; => #t
(type p)                              ; => :point
(equal? (make-point 1 2) (make-point 1 2))  ; => #t
```

## Multimethods

Clojure-style polymorphic dispatch based on a user-defined dispatch function.

### `defmulti`

Evaluate a dispatch function and bind a new multimethod to `name`. Each call to
the multimethod passes all call arguments to the dispatch function, then uses
its result to select a method. `defmulti` returns `nil`.

```sema
(defmulti area (fn (shape) (get shape :type)))
```

### `defmethod`

Evaluate the dispatch value and handler, then add the handler to an existing
multimethod. Use `:default` as the dispatch value for a fallback handler. It is
an error if the named binding is not a multimethod. `defmethod` returns `nil`.

```sema
(defmethod area :circle
  (fn (shape) (* 3.14159 (expt (get shape :radius) 2))))

(defmethod area :rect
  (fn (shape) (* (get shape :width) (get shape :height))))

(defmethod area :default
  (fn (shape) (throw "unknown shape")))

(area {:type :circle :radius 5})       ; => 78.53975
(area {:type :rect :width 3 :height 4}) ; => 12
```

## Loading Files

### `load`

Evaluate the path, load the Sema source file, and execute it in the current
environment. Unlike `import`, `load` does not use module exports: top-level
definitions become available in the current scope. The path is resolved
relative to the current source file when possible.

```sema
(load "helpers.sema")                  ; execute file, bindings available here
```

### `eval`

Evaluate one data structure as code in the current environment. See
[Metaprogramming](./macros-modules.md#eval).

```sema
(eval '(+ 1 2))                        ; => 3
(eval (read "(* 3 4)"))                ; => 12
```

## Error Handling

### `try` / `catch`

Catch errors with structured error maps.

```sema
(try
  (/ 1 0)
  (catch e
    (println (format "Error: ~a" (:message e)))
    (:type e)))        ; => :eval
```

::: warning
`try`/`catch` catches **all** error types — not just user exceptions thrown with `throw`. This includes internal errors like `:unbound` (typos in variable names), `:permission-denied`, and `:arity` (wrong number of arguments). Catching everything can silently mask bugs. **Re-throw errors you don't intend to handle.**
:::

There is no `finally` clause; the last form must be `(catch e ...)`. For cleanup
that must run on both paths, catch, clean up, and re-throw, or use
[`guard`](#guard).

#### Error map fields

Every caught error is a map with at least `:type`, `:message`, and `:stack-trace`. User-thrown values appear under `:value`, and some error types include additional fields:

| `:type` | Description | Extra fields |
|---|---|---|
| `:reader` | Syntax / parse error | — |
| `:eval` | General evaluation error | — |
| `:type-error` | Wrong argument type | `:expected`, `:got` |
| `:arity` | Wrong number of arguments | — |
| `:unbound` | Undefined variable | `:name` |
| `:llm` | LLM provider error | — |
| `:io` | File / network I/O error | — |
| `:permission-denied` | Sandboxed capability denied | `:function`, `:capability` |
| `:user` | Thrown with `throw` | `:value` (the original thrown value) |

#### Discriminating error types

Use the `:type` field to handle specific errors and re-throw the rest:

```sema
(try
  (some-operation)
  (catch e
    (cond
      ((= (get e :type) :permission-denied)
       (println "Access denied!"))
      ((= (get e :type) :user)
       (println (format "User error: ~a" (get e :message))))
      (else
       (throw e)))))  ;; re-throw unexpected errors
```

### `throw`

Throw any value as an error.

```sema
(throw "something went wrong")
(throw {:code 404 :reason "not found"})
```

### `raise`

R7RS `raise`: signal an arbitrary object as an exception. Identical in effect to `throw`, but it's a first-class procedure, so it can be passed to higher-order code where a special form cannot go.

```sema
(try (raise 42) (catch e (:value e)))          ; => 42
(try (raise {:code 404}) (catch e (:value e))) ; => {:code 404}
```

Unlike `error` (which takes a message string), `raise` signals the object itself — any value, not just a string.

### `guard`

R7RS structured exception handling. `guard` is a prelude macro built from
`try`/`catch` and `cond`, not an evaluator special form.

```sema
(guard (var clause ...) body ...)
```

Evaluates the body; if nothing is raised, `guard` returns the body's last value. If an error is raised — via `raise`/`throw` **or** a native runtime error — it is bound to `var` and the clauses are tried exactly like `cond`, with an optional `else`:

```sema
(guard (e ((string? e) (str "caught: " e))
          (else :unknown))
  (raise "boom"))                      ; => "caught: boom"

(guard (e ((number? e) (* 2 e)))
  100)                                 ; => 100 (no raise — clauses never run)
```

For `(raise obj)` / `(throw obj)`, `var` is bound to the raised object itself. A native runtime error (division by zero, unbound variable, `(error "msg")`) has no raw raised object, so `var` is the same error map `try`/`catch` produces — discriminate with `(:type e)` / `(:message e)`, gating on `(map? e)` first if a raw raised value could also reach the clause:

```sema
(guard (e (else (:message e)))
  (/ 1 0))                             ; => "division by zero"
```

If no clause matches and there is no `else`, the condition is **re-raised** rather than swallowed — an outer `guard` (or `try`) recovers the same object:

```sema
(guard (outer ((number? outer) (* 10 outer)))
  (guard (e ((string? e) e))           ; 7 is not a string — no match
    (raise 7)))                        ; => 70 (re-raised to the outer guard)
```

This makes `guard` safer than a catch-all `try`/`catch`: conditions you didn't anticipate propagate instead of being silently absorbed.

::: tip
`(car '())` and `(first [])` return `nil` in Sema (a deliberate safe-accessor deviation from R7RS), so they don't raise — `guard` never fires on them.
:::

## Async / Await

### `async`

Create an async task that evaluates `body` cooperatively and returns a promise.

```
(async body ...)
```

The task runs on the VM's cooperative scheduler. Multiple async tasks interleave at yield points (channel operations, await, sleep).

```sema
(define p (async (+ 1 2)))
(await p)  ; => 3
```

### `await`

Wait for an async promise to resolve and return its value.

```
(await promise)
```

If the promise was rejected, raises an error. Inside an async task, `await` yields to the scheduler allowing other tasks to run. At the top level, `await` runs the scheduler until the promise resolves.

```sema
(let ((p1 (async (* 3 3)))
      (p2 (async (* 4 4))))
  (+ (await p1) (await p2)))  ; => 25
```
