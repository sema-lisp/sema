---
name: "text/trim-indent"
module: "text-processing"
section: "Text Cleaning"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["text/clean-whitespace", "string/lines", "string/trim-left"]
---

Remove common leading indentation from all lines.

```sema
(text/trim-indent "    hello\n    world")   ; => "hello\nworld"
(text/trim-indent "    hello\n      world") ; => "hello\n  world"
```
