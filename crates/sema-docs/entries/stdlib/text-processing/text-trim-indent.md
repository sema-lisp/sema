---
name: "text/trim-indent"
module: "text-processing"
section: "Text Cleaning"
---

Remove common leading indentation from all lines.

```sema
(text/trim-indent "    hello\n    world")   ; => "hello\nworld"
(text/trim-indent "    hello\n      world") ; => "hello\n  world"
```
