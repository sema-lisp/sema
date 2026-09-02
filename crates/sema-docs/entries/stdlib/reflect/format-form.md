---
name: "format/form"
module: "reflect"
section: "Reflection"
params: [{ name: form, type: any }]
returns: "string"
see_also: ["read/string", "read/all", "str"]
---

Pretty-print a form to canonical Sema source using the formatter. The argument `form` is a value, usually one produced by `read/string` or `read/all`, or a quoted literal. The result is a string with normalized whitespace and indentation, the same output `sema fmt` produces for that form. Use it to print generated or rewritten code as source text.

```sema
(format/form (read/string "(define  x  1)"))   ; => "(define x 1)"
(format/form '(+ 1 2))                          ; => "(+ 1 2)"
```
