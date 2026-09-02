---
name: "text/normalize-newlines"
module: "text-processing"
section: "Text Cleaning"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["text/clean-whitespace", "string/lines", "text/trim-indent"]
---

Convert `\r\n` (Windows) and `\r` (old Mac) line endings to `\n` (Unix).

```sema
(text/normalize-newlines "line1\r\nline2\rline3")
; => "line1\nline2\nline3"
```
