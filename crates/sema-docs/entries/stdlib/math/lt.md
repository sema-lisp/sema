---
name: "<"
module: "math"
section: "Comparison"
syntax: "(< num ...)"
returns: "bool"
---

Less than. Supports chaining.

```sema
(< 1 2)       ; => #t
(< 1 2 3)     ; => #t
(< 3 2)       ; => #f
```
