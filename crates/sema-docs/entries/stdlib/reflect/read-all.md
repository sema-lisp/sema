---
name: "read/all"
module: "reflect"
section: "Reflection"
params: [{ name: s, type: string }]
returns: "list"
see_also: ["read/string", "eval", "format/form"]
---

Parse every top-level form in a string and return them as a list. The argument `s` is a string of Sema source; it may hold zero or more forms separated by whitespace, and an empty string returns an empty list. Each element of the result is one unevaluated form, in source order, the same shape `read/string` returns for a single form. A malformed form raises a reader error.

```sema
(read/all "(a) (b)")   ; => ((a) (b))
(read/all "1 2 3")     ; => (1 2 3)
(read/all "")          ; => ()
```
