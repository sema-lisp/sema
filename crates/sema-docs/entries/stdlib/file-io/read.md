---
name: "read"
module: "file-io"
section: "Reader"
params: [{ name: s, type: string }]
---

Parse a string containing a single Sema expression and return it as data (unevaluated).

```sema
(read "(+ 1 2)")   ; => (+ 1 2)
(read "42")        ; => 42
```
