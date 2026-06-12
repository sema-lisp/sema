---
name: "path/absolute?"
module: "file-io"
section: "Path Manipulation"
---

Test if a path is absolute.

```sema
(path/absolute? "/usr/bin")   ; => #t
(path/absolute? "relative")  ; => #f
```
