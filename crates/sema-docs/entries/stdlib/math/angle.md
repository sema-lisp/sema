---
name: "angle"
module: "math"
section: "Complex Polar Conversion"
params: [{ name: z, type: number }]
returns: "number"
see_also: ["magnitude", "make-polar", "make-rectangular", "real-part", "imag-part"]
---

Return the angle (argument) of a complex number in radians, in the range (-π, π]. For a complex number a+bi this is `atan2(b, a)`; a positive real returns `0.0` and a negative real returns π. The result is always inexact — it comes out of a floating-point `atan2`, so even exact inputs produce a float.

```sema
(angle 3+4i)   ; => 0.9272952180016122   ; atan2(4, 3)
(angle 5)      ; => 0.0                   ; positive real
(angle -5)     ; => 3.141592653589793     ; π
(angle 0+1i)   ; => 1.5707963267948966    ; π/2
```

Together with [`magnitude`](#magnitude) this gives the polar form of a number,
and [`make-polar`](#make-polar) reconstructs a complex number from magnitude and
angle.
