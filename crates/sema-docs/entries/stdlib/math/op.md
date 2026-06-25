---
name: "-"
module: "math"
section: "Basic Arithmetic"
syntax: "(- num ...)"
returns: "number"
---

Subtract numbers. With one argument, negates. With multiple, subtracts left to right.

```sema
(- 10 3)      ; => 7
(- 10 3 2)    ; => 5
(- 5)         ; => -5
```
