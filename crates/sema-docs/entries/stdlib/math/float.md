---
name: "float"
module: "math"
params: [{ name: x, type: "number | string" }]
returns: "float"
---

Convert a number or numeric string to a float. Signals an error if a string cannot be parsed as a float.

```sema
(float 5)      ; => 5.0
(float "3.5")  ; => 3.5
```
