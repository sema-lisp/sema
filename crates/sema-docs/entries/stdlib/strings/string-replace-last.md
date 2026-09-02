---
name: "string/replace-last"
module: "strings"
section: "Replacement"
params: [{ name: s, type: string }, { name: from, type: string }, { name: to, type: string }]
returns: "string"
see_also: ["string/replace", "string/replace-first", "regex/replace"]
---

Replace only the last occurrence of a substring (a literal match, not a regex). The mirror of `string/replace-first`; use `string/replace` to replace all.

```sema
(string/replace-last "aaa" "a" "b")     ; => "aab"
(string/replace-last "a.b.c" "." "-")   ; => "a.b-c"
```
