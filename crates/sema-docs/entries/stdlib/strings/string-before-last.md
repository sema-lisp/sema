---
name: "string/before-last"
module: "strings"
section: "Slicing & Extraction"
params: [{ name: s, type: string }, { name: needle, type: string }]
returns: "string"
see_also: ["string/before", "string/after-last", "string/between"]
---

Everything before the *last* occurrence of a needle (handy for grabbing a directory from a path). Returns the original string if the needle isn't found. Use `string/before` to split on the first occurrence instead.

```sema
(string/before-last "a.b.c" ".")       ; => "a.b"
(string/before-last "/a/b/c.txt" "/")  ; => "/a/b"
(string/before-last "no-dot" ".")      ; => "no-dot"   (needle not found)
```
