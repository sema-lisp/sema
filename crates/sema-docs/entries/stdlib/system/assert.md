---
name: "assert"
module: "system"
section: "Errors"
params: [{ name: condition, type: any }, { name: message, type: string }]
---

Raise an error if `condition` is falsy, otherwise return `#t`. An optional second argument supplies the error message (default `"assertion failed"`).

```sema
(assert (> 2 1))               ; => #t
(assert (= 1 2) "must match")  ; raises "must match"
```
