---
name: "guard"
module: "special-forms"
syntax: "(guard (var clause ...) body ...)"
---

R7RS-style structured exception handling. Evaluates the `body` expressions; if no error occurs, `guard` returns the value of the last one. If an error is raised (via `throw` or a native runtime error), it is bound to `var` and the `clause`s are evaluated exactly like `cond` — the first clause whose test is truthy has its body evaluated and returned, and an `else` clause (if present, and last) always matches.

If none of the clauses match and there is no `else`, the condition is **re-raised** from `guard`'s own position rather than swallowed.

```sema
(guard (e ((string? (:value e)) (:value e))
          (else :unknown))
  (throw "boom"))
;; => "boom"
```

```sema
(guard (e ((number? (:value e)) (* 2 (:value e))))
  100)
;; => 100 (body did not raise, clauses are never evaluated)
```

```sema
(guard (e ((= (:value e) 1) :one))
  (throw 2))
;; => propagates "User exception: 2" — no clause matched and there is no `else`
```

As with `try`/`catch`, `var` is bound to Sema's error map (`{:type ... :message ... :value ...}`), not the raw raised value — there is no bare `raise` in Sema, only `throw`. Use `(:value var)` to read the value passed to `(throw x)`; use `(:type var)` / `(:message var)` to discriminate native errors such as division by zero, unbound variables, or `(error "msg")`:

```sema
(guard (e (else (:message e)))
  (/ 1 0))
;; => "division by zero" (a native error, not a `throw`, is still caught)
```

**Note:** `guard` expands to `try`/`catch` wrapping a `cond` and is defined in the prelude; it does not require an import. Because it desugars to `try`, re-raising re-wraps the condition (an outer `catch`/`guard` sees the re-raised value one `:value` deeper) — the same behavior as manually re-`throw`ing inside a `catch` handler.
