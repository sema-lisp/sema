---
name: "promise-forced?"
module: "predicates"
section: "Promise Predicates"
---

Test if a promise has been forced (evaluated).

```sema
(define p (delay (+ 1 2)))
(promise-forced? p)   ; => #f
(force p)
(promise-forced? p)   ; => #t
```
