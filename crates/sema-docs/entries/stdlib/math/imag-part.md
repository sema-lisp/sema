---
name: "imag-part"
module: "math"
section: "Complex Accessors"
params: [{ name: z, type: number }]
returns: "number"
see_also: ["real-part", "make-rectangular", "magnitude", "angle"]
---

Return the imaginary part of a complex number. For any real number (integer, rational, or float) the imaginary part is exact `0`. The result preserves the exactness of the stored component, so an exact complex yields an exact imaginary part.

```sema
(imag-part 3+4i)   ; => 4
(imag-part 5i)     ; => 5
(imag-part 2.5)    ; => 0    ; real number
(imag-part 1/3)    ; => 0
```

The companion accessor is [`real-part`](../real-part); together they invert
[`make-rectangular`](../make-rectangular).
