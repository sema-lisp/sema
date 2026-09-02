---
name: "string/trim-right"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/trim", "string/trim-left", "string/chop-end"]
---

Remove trailing whitespace only. See `string/trim-left` for the leading side and `string/trim` for both.

```sema
(string/trim-right "hi  ")     ; => "hi"
(string/trim-right "  hi  ")   ; => "  hi"   ; leading space kept
```
