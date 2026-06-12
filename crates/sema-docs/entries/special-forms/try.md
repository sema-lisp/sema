---
name: "try"
module: "special-forms"
syntax: "(try body ... (catch var handler ...))"
---

Evaluate a sequence of body expressions and, if any error occurs, catch it and evaluate the handler expressions. The last argument must be a catch clause of the form `(catch var handler ...)`, where `var` is a symbol that binds the caught error. All body expressions except the catch clause are evaluated in order; if none raise an error, the result of the last body expression is returned. If an error occurs, evaluation stops, the error is bound to `var` in a fresh child environment, and the handler expressions are evaluated.

The caught error is a map value with at least the following keys:
- `:type` — a keyword indicating the error category (e.g. `:user`, `:eval`, `:type-error`, `:arity`, `:unbound`, `:permission-denied`, `:reader`, `:llm`, `:io`)
- `:message` — a human-readable description string
- `:stack-trace` — a list of stack frames with `:name`, `:file`, `:line`, and `:col`

Additional keys may be present depending on the error type. For example, `:user` errors (thrown with `throw`) include `:value` containing the original thrown value. `:type-error` includes `:expected` and `:got`. `:unbound` includes `:name`.

```sema
(try
  (/ 1 0)
  (catch e
    (println "Error:" (:message e))
    (:type e)))
; => :eval
```

Use the `:type` field to discriminate specific errors and re-throw anything you do not intend to handle. Catching all errors can silently mask bugs such as typos in variable names or arity mismatches.

```sema
(try
  (some-operation)
  (catch e
    (cond
      ((= (:type e) :permission-denied)
       (println "Access denied!"))
      ((= (:type e) :user)
       (println "User error:" (:message e)))
      (else
       (throw e)))))
```

The handler body supports tail-call optimization on its last expression, just like a regular `begin` block.

**Warning:** `try` catches **all** error types, not just user exceptions thrown with `throw`. This includes internal errors like `:unbound` (undefined variables), `:arity` (wrong number of arguments), and `:permission-denied` (sandbox violations). Always re-throw errors you do not intend to handle.
