---
name: "unless"
module: "special-forms"
syntax: "(unless condition body ...)"
---

Inverse of `when`: evaluates its body expressions only when `condition` is falsy. If the condition is truthy, `unless` returns `nil` without evaluating any body expression. It accepts any number of body expressions and is useful for guard clauses or early-exit style code.

Like all conditionals in Sema, only `nil` and `#f` are falsy; every other value is truthy. Use `unless` when you want to express "do this only if NOT" without nesting an `if` or swapping a predicate.

```sema
(unless (> x 0) (println "non-positive"))
```

```sema
(unless (authenticated? user)
  (println "Access denied")
  (return 403))
```

```sema
(unless nil
  (println "nil is falsy, so this prints"))   ; prints and returns nil
```

```sema
(unless #t
  (println "skipped"))   ; => nil (#t is truthy, body skipped)
```
