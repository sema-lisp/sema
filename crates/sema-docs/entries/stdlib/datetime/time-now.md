---
name: "time/now"
module: "datetime"
section: "Current Time"
---

Return the current time as a UTC Unix timestamp in seconds, with fractional milliseconds.

```sema
(time/now)   ; => 1707955200.123
```

The integer part is seconds since the Unix epoch; the fractional part provides millisecond precision.

```sema
(define now (time/now))
(println "Current timestamp: " now)

;; Extract just the seconds (truncate fractional part)
(define whole-seconds (floor now))
```
