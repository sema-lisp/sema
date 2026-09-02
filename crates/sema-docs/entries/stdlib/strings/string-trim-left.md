---
name: "string/trim-left"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/trim", "string/trim-right", "string/chop-start"]
---

Remove leading whitespace only. See `string/trim-right` for the trailing side and `string/trim` for both.

```sema
(string/trim-left "  hi")     ; => "hi"
(string/trim-left "  hi  ")   ; => "hi  "   ; trailing space kept
```
