---
name: "path/absolute?"
module: "file-io"
section: "Path Manipulation"
params: [{ name: path, type: string }]
returns: "bool"
see_also: ["path/absolute", "path/canonicalize"]
---

Test if a path is absolute.

```sema
(path/absolute? "/usr/bin")   ; => #t
(path/absolute? "relative")  ; => #f
```
