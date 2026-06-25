---
name: "str"
module: "strings"
section: "Scheme Compatibility Aliases"
syntax: "(str value ...)"
returns: "string"
---

Convert any value to its string representation.

```sema
(str 42)           ; => "42"
(str #t)           ; => "#t"
(str '(1 2 3))    ; => "(1 2 3)"
```
