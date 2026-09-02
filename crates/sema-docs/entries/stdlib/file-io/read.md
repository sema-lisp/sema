---
name: "read"
module: "file-io"
section: "Reader"
params: [{ name: s, type: string }]
returns: "any"
see_also: ["read-many", "eval", "read-line"]
---

Parse a string containing a single Sema expression and return it as data (unevaluated). Because Sema is homoiconic, the result is an ordinary value (a list, symbol, number, ...) that you can inspect or hand to `eval`.

`read` returns only the **first** datum and ignores any trailing input — `(read "1 2 3")` is `1`, not an error. Use `read-many` when the string may hold several expressions.

```sema
(read "(+ 1 2)")        ; => (+ 1 2)   (a 3-element list, not 3)
(eval (read "(+ 1 2)")) ; => 3         (parse, then evaluate)
(read "42")             ; => 42
```
