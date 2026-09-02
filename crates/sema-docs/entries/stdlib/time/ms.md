---
name: "time/ms"
module: "time"
params: [{ name: thunk, type: function }]
returns: "float"
see_also: ["time", "time-ms", "sys/elapsed"]
---

Call a zero-argument thunk and return how long it took to run, in milliseconds (as a float). Useful for quick timing.

```sema
(time/ms (lambda () (apply + (range 1000))))   ; => 0.045  (varies per run)
```

The thunk's return value is discarded — `time/ms` reports only the elapsed
wall-clock time, so put the work you want to measure inside the lambda. For
nanosecond-resolution micro-benchmarks prefer `sys/elapsed` (returns nanoseconds
as an int), since a single sub-millisecond run rounds noisily here.
