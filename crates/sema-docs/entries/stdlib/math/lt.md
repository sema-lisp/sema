---
name: "<"
module: "math"
section: "Comparison"
syntax: "(< num ...)"
returns: "bool"
see_also: ["<=", ">", ">=", "="]
---

Less than. Variadic and chaining: `(< a b c)` is true when the arguments are in strictly ascending order (each `< ` the next). Also orders same-typed non-numbers (e.g. strings lexicographically), but errors on mixed or unorderable types — unlike `=`, which compares anything structurally and never errors.

```sema
(< 1 2)         ; => #t
(< 1 2 3)       ; => #t   (each < the next)
(< 1 3 2)       ; => #f   (chain breaks at 3 < 2)
(< "a" "b")     ; => #t   (string ordering)
```
