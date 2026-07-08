---
name: "float"
module: "math"
params: [{ name: x, type: "number | string" }]
returns: "float"
---

Convert a number or numeric string to a float. Any real number works — bignums and exact rationals project to the nearest float (like `exact->inexact`); complex numbers are rejected (no real projection). Signals an error if a string cannot be parsed as a float.

```sema
(float 5)      ; => 5.0
(float "3.5")  ; => 3.5
(float 1/2)    ; => 0.5
```
