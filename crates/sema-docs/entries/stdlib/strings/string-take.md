---
name: "string/take"
module: "strings"
section: "Slicing & Extraction"
params: [{ name: s, type: string }, { name: n, type: integer }]
returns: "string"
see_also: ["string/slice", "string/truncate-width"]
---

Take the first N characters (positive N) or last N characters (negative N). Unlike `string/slice`, N is a *length*, not an index, and it clamps: asking for more characters than exist returns the whole string instead of raising.

```sema
(string/take "hello" 3)    ; => "hel"
(string/take "hello" -2)   ; => "lo"
(string/take "hi" 10)      ; => "hi"   ; clamps, never errors
```
