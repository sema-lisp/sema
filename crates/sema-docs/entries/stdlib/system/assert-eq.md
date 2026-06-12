---
name: "assert="
module: "system"
section: "Errors"
params: [{ name: expected, type: any }, { name: actual, type: any }]
---

Raise an error if `expected` and `actual` are not equal, with a message showing both values. Returns `#t` when they match.

```sema
(assert= 4 (+ 2 2))   ; => #t
(assert= 4 5)         ; raises "assertion failed: expected 4, got 5"
```
