---
name: "int"
module: "math"
params: [{ name: x, type: "number | string" }]
returns: "int"
---

Convert a number or numeric string to an integer. Floats and exact rationals are truncated **toward zero** (the fractional part is dropped, not rounded); exact integers of any size pass through unchanged, so bignums stay exact. Signals an error if a string cannot be parsed as an integer.

Truncation differs from `floor` on negatives: `int` chops toward zero while `floor` always rounds down. `(int -3.9)` is `-3` (an exact integer), but `(floor -3.9)` is `-4.0` (`floor` rounds down and preserves inexactness, so a float stays a float). Use `int` for "drop the decimals and give me an integer", `floor`/`ceil` when you need a specific rounding direction.

```sema
(int 3.9)     ; => 3
(int -3.9)    ; => -3    (toward zero, not -4)
(int "42")    ; => 42
(floor -3.9)  ; => -4.0  (contrast: rounds down, stays a float)
```
