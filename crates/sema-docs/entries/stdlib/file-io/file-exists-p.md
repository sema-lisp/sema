---
name: "file/exists?"
module: "file-io"
section: "File Predicates"
params: [{ name: path, type: string }]
returns: "bool"
---

Test if a file or directory exists.

```sema
(file/exists? "data.txt")   ; => #t or #f
```
