---
name: "sys/elapsed"
module: "system"
section: "Process Information"
syntax: "(sys/elapsed)"
returns: "int"
see_also: ["time-ms", "time/ms", "time"]
---

Return the time since the process started, in **nanoseconds**, as an integer.
It reads a monotonic clock, so the value never goes backwards and is not
affected by clock adjustments; that makes two readings safe to subtract for
timing a section of code. Divide by 1000000 for milliseconds.

Prefer `time` to time a single expression, and `time/now` when you need a
wall-clock timestamp rather than a duration.

```sema
(sys/elapsed)   ; => varies, e.g. 482937100

;; Time a section by subtracting two readings.
(define start (sys/elapsed))
(list/sum (range 0 100000))
(define ms (/ (- (sys/elapsed) start) 1000000.0))
```
