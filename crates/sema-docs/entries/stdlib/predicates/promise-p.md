---
name: "promise?"
module: "predicates"
section: "Promise Predicates"
---

Test if a value is a promise (created with `delay`).

```sema
(promise? (delay 1))   ; => #t
(promise? 42)          ; => #f
```
