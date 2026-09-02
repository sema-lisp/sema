---
name: "read/string"
module: "reflect"
section: "Reflection"
params: [{ name: s, type: string }]
returns: "any"
see_also: ["read/all", "eval", "format/form"]
---

Parse exactly one Sema form from a string and return it as data (a quoted form). The argument `s` is a string that holds one complete form; an empty string returns `nil`, and a malformed form raises a reader error. The result is the parsed value — a list, symbol, number, string, or other literal — which is not evaluated. Pass it to `eval` to run it, or to `format/form` to print it as canonical source.

```sema
(read/string "(+ 1 2)")   ; => (+ 1 2)
(read/string "42")        ; => 42
(eval (read/string "(+ 1 2)"))   ; => 3
```
