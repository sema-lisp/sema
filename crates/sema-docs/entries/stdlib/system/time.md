---
name: "time"
module: "system"
section: "Timing"
params: [{ name: thunk, type: function }]
---

Call the zero-argument `thunk`, print the elapsed wall-clock time (in milliseconds) to standard error, and return the thunk's result. See `time/ms` to capture the duration as a value instead.

```sema
(time (fn () (+ 1 2)))   ; prints "Elapsed: 0.003ms", returns 3
```
