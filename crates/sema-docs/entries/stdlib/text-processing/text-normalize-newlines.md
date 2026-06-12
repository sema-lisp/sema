---
name: "text/normalize-newlines"
module: "text-processing"
section: "Text Cleaning"
---

Convert `\r\n` (Windows) and `\r` (old Mac) line endings to `\n` (Unix).

```sema
(text/normalize-newlines "line1\r\nline2\rline3")
; => "line1\nline2\nline3"
```
