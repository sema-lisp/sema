---
name: "when"
module: "special-forms"
syntax: "(when condition body ...)"
---

Single-branch conditional that evaluates its body expressions only when `condition` is truthy. If the condition is falsy, `when` returns `nil` without evaluating any body expression. Unlike `if`, `when` accepts any number of body expressions and always returns `nil` when the condition is falsy.

Use `when` when you need a side-effecting block that should run only under a certain condition and do not need an else branch. Because the body is not evaluated unless the condition holds, it is safe to use for guarded computations.

```sema
(when (> x 0) (println "positive"))
```

```sema
(when (file-exists? path)
  (println "Loading config...")
  (load path)
  (println "Done."))
```

```sema
(when #f
  (println "this never prints"))   ; => nil
```

```sema
(define n 7)
(when (even? n)
  (println "even")
  (* n 2))        ; => nil (n is odd, body skipped)
```
