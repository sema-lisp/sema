---
name: "assert"
module: "system"
section: "Errors"
params: [{ name: condition, type: any }, { name: message, type: string }]
returns: "bool"
see_also: ["assert=", "error", "raise"]
---

Raise an error if `condition` is falsy, otherwise return `#t`. An optional second argument supplies the error message (default `"assertion failed"`).

```sema
(assert (> 2 1))               ; => #t
(assert (= 1 2) "must match")  ; raises "must match"
```

For equality checks, prefer `assert=` — it reports both the expected and actual value in the failure message, which `assert` cannot.
