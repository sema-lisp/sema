---
name: "truncate"
module: "math"
section: "Scheme Aliases"
---

Drop the fractional part, rounding toward zero (so negatives round *up*). Unlike [`round`](#round), it never inspects the fraction — `3.9` truncates to `3`. Exactness-preserving: a float input truncates to a float, an exact rational truncates to an exact integer.

```sema
(truncate 3.7)  ; => 3.0
(truncate -3.7) ; => -3.0  ; toward zero, not floor (-4.0)
(truncate 3.9)  ; => 3.0
(truncate 7/2)  ; => 3     (exact rational -> exact integer)
```
