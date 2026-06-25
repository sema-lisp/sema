---
name: "iota"
module: "lists"
section: "Construction"
syntax: "(iota count [start [step]])"
returns: "list"
---

Generate a list of numbers. `(iota count)`, `(iota count start)`, or `(iota count start step)`.

```sema
(iota 5)         ; => (0 1 2 3 4)
(iota 3 10)      ; => (10 11 12)
(iota 4 0 2)     ; => (0 2 4 6)
```
