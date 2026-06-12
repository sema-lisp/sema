---
name: "time/diff"
module: "datetime"
section: "Arithmetic"
---

Compute the difference between two timestamps in seconds. Returns `t1 - t2` (the first argument minus the second). The result can be negative.

```sema
(time/diff t1 t2) ; => float (seconds)
```

```sema
(define morning 1736935800.0)   ; 2025-01-15 10:10:00 UTC
(define afternoon 1736943000.0) ; 2025-01-15 12:10:00 UTC

(time/diff afternoon morning)  ; =>  7200.0  (2 hours)
(time/diff morning afternoon)  ; => -7200.0  (negative — morning is earlier)
(time/diff morning morning)    ; =>  0.0
```

`time/diff` returns a signed value: positive when `t1 > t2`, negative when `t1 < t2`. Use `abs` if you need the absolute elapsed time regardless of order.
