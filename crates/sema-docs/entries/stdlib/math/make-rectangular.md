---
name: "make-rectangular"
module: "math"
section: "Complex Construction"
params: [{ name: real, type: number }, { name: imag, type: number }]
returns: "number"
see_also: ["make-polar", "real-part", "imag-part", "magnitude", "angle"]
---

Construct a complex number from a real part and an imaginary part. If the imaginary part is *exact* zero (and the real part is itself real), the result collapses to just the real part; an *inexact* zero imaginary part (`0.0`) keeps the number complex, since exactness is part of its identity.

```sema
(make-rectangular 3 4)       ; => 3+4i
(make-rectangular 1/3 1/2)   ; => 1/3+1/2i
(make-rectangular 3.5 2.0)   ; => 3.5+2.0i
(make-rectangular 2 0)       ; => 2       ; exact-zero imaginary collapses
(make-rectangular 3 0.0)     ; => 3+0.0i  ; inexact zero stays complex
```

[`make-polar`](../make-polar) is the polar-coordinate constructor;
[`real-part`](../real-part) and [`imag-part`](../imag-part) take the pieces back out.
