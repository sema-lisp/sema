---
name: "string/remove"
module: "strings"
section: "Replacement"
params: [{ name: s, type: string }, { name: sub, type: string }]
returns: "string"
see_also: ["string/replace", "string/chop-start", "string/chop-end"]
---

Remove all occurrences of a literal substring. Equivalent to `(string/replace s sub "")`.

```sema
(string/remove "hello world" "o")    ; => "hell wrld"
(string/remove "a-b-c" "-")          ; => "abc"
```
