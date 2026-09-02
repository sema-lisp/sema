---
name: "string/trim"
module: "strings"
section: "Core String Operations"
aliases: ["string-trim"]
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/trim-left", "string/trim-right", "text/clean-whitespace"]
---

Remove whitespace from both ends.

```sema
(string/trim "  hello  ")   ; => "hello"
(string/trim "\thello\n")   ; => "hello"
```
