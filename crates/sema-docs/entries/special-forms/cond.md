---
name: "cond"
module: "special-forms"
syntax: "(cond (test body ...) ... [(else body ...)])"
---

Multi-branch conditional that evaluates tests in order until one is truthy, then evaluates the corresponding body and returns the last body's value. Each clause is a list where the first element is a test expression and the remaining elements are body expressions. The `else` clause, if present, acts as a catch-all and must be the last clause.

If a clause contains only a test with no body (e.g., `(predicate)`), `cond` returns `#t` when that test is truthy. If no clause matches and there is no `else`, `cond` returns `nil`. Like `if`, `cond` is a special form and evaluates only the selected branch.

```sema
(cond
  ((< x 0) "negative")
  ((= x 0) "zero")
  (else "positive"))
```

```sema
(define score 85)
(cond
  ((>= score 90) "A")
  ((>= score 80) "B")
  ((>= score 70) "C")
  (else "F"))
```

```sema
(cond
  ((> 5 10) "unreachable")
  ((< 3 7)))          ; => #t (test-only clause)
```

```sema
(cond
  ((= 1 2) "no")
  ((= 3 4) "also no"))  ; => nil (no match, no else)
```
