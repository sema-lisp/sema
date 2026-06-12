---
name: "string/take"
module: "strings"
section: "Slicing & Extraction"
---

Take the first N characters (positive) or last N characters (negative).

```sema
(string/take "hello" 3)   ; => "hel"
(string/take "hello" -2)  ; => "lo"
```
