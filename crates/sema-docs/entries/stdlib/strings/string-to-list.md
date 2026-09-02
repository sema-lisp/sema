---
name: "string/to-list"
module: "strings"
section: "Type Conversions"
aliases: ["string->list"]
params: [{ name: s, type: string }]
returns: "list"
see_also: ["list->string", "string/chars", "string/to-char"]
---

Convert a string to a list of characters.

```sema
(string/to-list "abc")   ; => (#\a #\b #\c)
```
