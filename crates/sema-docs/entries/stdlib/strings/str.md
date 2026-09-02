---
name: "str"
module: "strings"
section: "Scheme Compatibility Aliases"
syntax: "(str value ...)"
returns: "string"
see_also: ["string/append", "format", "number/to-string"]
---

Convert each argument to its display string and concatenate them. With one argument it is just "value → string"; with several it is a handy alternative to `string/append` that stringifies non-string args for you.

Strings come out as their raw contents (no surrounding quotes) — this is the *display* form, like `~a` in `format`, not the readable `~s` form.

```sema
(str 42)           ; => "42"
(str #t)           ; => "#t"
(str '(1 2 3))     ; => "(1 2 3)"
(str "id-" 42)     ; => "id-42"
```
