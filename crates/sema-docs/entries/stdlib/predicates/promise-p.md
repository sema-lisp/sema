---
name: "promise?"
module: "predicates"
section: "Promise Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["promise-forced?", "force", "delay"]
---

Test if a value is a promise (created with `delay`).

```sema
(promise? (delay 1))   ; => #t
(promise? 42)          ; => #f
```
