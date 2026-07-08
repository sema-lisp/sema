---
name: "real-part"
module: "math"
section: "Complex Accessors"
params: [{ name: z, type: number }]
returns: "number"
see_also: ["imag-part", "make-rectangular", "magnitude", "angle"]
---

Return the real part of a complex number. For a real number (integer, rational, or float) it returns the number itself unchanged; for a pure-imaginary value the real part is exact `0`. Exactness of the stored component is preserved.

```sema
(real-part 3+4i)   ; => 3
(real-part 5i)     ; => 0     ; pure imaginary
(real-part 2.5)    ; => 2.5   ; real number
(real-part 1/3)    ; => 1/3
```

The companion accessor is [`imag-part`](../imag-part); together they invert
[`make-rectangular`](../make-rectangular).
