---
name: "time/ms"
module: "time"
params: [{ name: thunk, type: function }]
returns: "float"
---

Call a zero-argument thunk and return how long it took to run, in milliseconds (as a float). Useful for quick timing.

```sema
(time/ms (lambda () (sum (range 1000))))   ; => 0.042
```
