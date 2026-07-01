---
name: "string/width"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "int"
---

Terminal **display width** of a string, in columns. Unlike `string-length` (which counts Unicode scalar values), this counts how many columns the string occupies when printed: wide characters (CJK, most emoji) count as 2, combining marks as 0, and ANSI escape sequences (colors, cursor moves) as 0. Use it for TUI layout, padding, and alignment, where character count is wrong for non-ASCII or styled text.

```sema
(string/width "hello")     ; => 5
(string/width "日本語")     ; => 6   (string-length is 3)
(string/width "👋")         ; => 2
(string/width "é")          ; => 1   (base + combining mark)
```
