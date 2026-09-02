---
name: "string/unwrap"
module: "strings"
section: "Prefix & Suffix"
params: [{ name: s, type: string }, { name: left, type: string }, { name: right, type: string, doc: "optional; defaults to left" }]
returns: "string"
see_also: ["string/wrap", "string/between", "string/chop-start"]
---

Remove surrounding delimiters if both present.

```sema
(string/unwrap "(hello)" "(" ")")  ; => "hello"
(string/unwrap "hello" "(" ")")    ; => "hello"
```
