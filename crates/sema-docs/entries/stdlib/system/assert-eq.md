---
name: "assert="
module: "system"
section: "Errors"
params: [{ name: expected, type: any }, { name: actual, type: any }]
returns: "bool"
see_also: ["assert", "error", "raise"]
---

Raise an error if `expected` and `actual` are not equal, with a message showing both values. Returns `#t` when they match. Equality is structural, so lists and maps compare by contents.

```sema
(assert= 4 (+ 2 2))               ; => #t
(assert= '(1 2 3) (list 1 2 3))   ; => #t  (structural, not identity)
(assert= 4 5)                     ; raises "assertion failed: expected 4, got 5"
```

Prefer `assert=` over `(assert (= a b))` when checking equality: it puts both values in the failure message, which `assert` cannot. Use plain `assert` for arbitrary boolean conditions.
