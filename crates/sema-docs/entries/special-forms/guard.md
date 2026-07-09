---
name: "guard"
module: "special-forms"
syntax: "(guard (var clause ...) body ...)"
---

R7RS-style structured exception handling. Evaluates the `body` expressions; if no error occurs, `guard` returns the value of the last one. If an error is raised (via `raise`/`throw` or a native runtime error), it is bound to `var` and the `clause`s are evaluated exactly like `cond` — the first clause whose test is truthy has its body evaluated and returned, and an `else` clause (if present, and last) always matches.

If none of the clauses match and there is no `else`, the condition is **re-raised** from `guard`'s own position rather than swallowed.

`var` is bound to the **raised object itself**. For `(raise obj)` / `(throw obj)` that is `obj` — clause tests read it directly:

```sema
(guard (e ((string? e) e)
          (else :unknown))
  (raise "boom"))
;; => "boom"
```

```sema
(guard (e ((number? e) (* 2 e)))
  100)
;; => 100 (body did not raise, clauses are never evaluated)
```

```sema
(guard (e ((= e 1) :one))
  (raise 2))
;; => propagates "User exception: 2" — no clause matched and there is no `else`
```

A native runtime error (division by zero, an unbound variable, `(error "msg")`, a type error) has no raw raised object, so `var` is Sema's error **map** (`{:type ... :message ... :value ...}`). Discriminate such errors with `(:type var)` / `(:message var)`, gating on `(map? var)` first since keyword access on a raw non-map raised value would itself error:

```sema
(guard (e (else (:message e)))
  (/ 1 0))
;; => "division by zero" (a native error, not a raise, is still caught)
```

**Note:** `guard` expands to `try`/`catch` wrapping a `cond` and is defined in the prelude; it does not require an import. It unwraps the `:user` error wrapper so `var` is the raw raised object, and re-raises `var` on no-match, so an outer `guard` recovers the same raw object (or the same native error map).

`(car '())` / `(first [])` return `nil` in Sema (a deliberate safe-accessor deviation from R7RS `car`), so they do **not** raise and `guard` never fires on them.
