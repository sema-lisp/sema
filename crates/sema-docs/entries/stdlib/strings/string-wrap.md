---
name: "string/wrap"
module: "strings"
section: "Prefix & Suffix"
params: [{ name: s, type: string }, { name: left, type: string }, { name: right, type: string, doc: "optional; defaults to left" }]
returns: "string"
see_also: ["string/unwrap", "string/between", "string/ensure-start"]
---

Wrap a string with left and right delimiters.

```sema
(string/wrap "hello" "(" ")")   ; => "(hello)"
(string/wrap "hello" "**")      ; => "**hello**"
```
