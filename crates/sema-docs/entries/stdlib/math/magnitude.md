---
name: "magnitude"
module: "math"
section: "Complex Polar Conversion"
params: [{ name: z, type: number }]
returns: "number"
see_also: ["angle", "make-polar", "make-rectangular", "real-part", "imag-part"]
---

Return the magnitude (modulus, absolute value) of a number. For a complex number a+bi this is `√(a²+b²)`, computed in floating point — so the magnitude of a complex number is inexact. For a real number it is just the absolute value and preserves exactness (an exact real stays exact).

```sema
(magnitude 3+4i)   ; => 5.0   ; √(9+16), inexact
(magnitude -3-4i)  ; => 5.0
(magnitude -5)     ; => 5     ; real: exact absolute value
(magnitude 1/3)    ; => 1/3
```

Pairs with [`angle`](../angle) to describe a number in polar form;
[`make-polar`](../make-polar) goes the other way.
