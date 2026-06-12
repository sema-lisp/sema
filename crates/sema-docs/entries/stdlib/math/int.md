---
name: "int"
module: "math"
params: [{ name: x, type: "number | string" }]
returns: "int"
---

Convert a number or numeric string to an integer. Floats are truncated toward zero. Signals an error if a string cannot be parsed as an integer.

```sema
(int 3.9)    ; => 3
(int "42")   ; => 42
```
