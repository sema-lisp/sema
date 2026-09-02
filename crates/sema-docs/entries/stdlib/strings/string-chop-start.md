---
name: "string/chop-start"
module: "strings"
section: "Prefix & Suffix"
params: [{ name: s, type: string }, { name: prefix, type: string }]
returns: "string"
see_also: ["string/chop-end", "string/ensure-start", "string/starts-with?"]
---

Remove a prefix if present, otherwise return unchanged. The inverse of `string/ensure-start`.

```sema
(string/chop-start "Hello World" "Hello ")  ; => "World"
(string/chop-start "Hello" "Bye")           ; => "Hello"
```
