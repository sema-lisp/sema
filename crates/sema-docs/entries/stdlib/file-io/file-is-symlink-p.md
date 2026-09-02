---
name: "file/is-symlink?"
module: "file-io"
section: "File Predicates"
params: [{ name: path, type: string }]
returns: "bool"
see_also: ["file/is-file?", "file/is-directory?", "path/canonicalize"]
---

Test if a path is a symbolic link.

```sema
(file/is-symlink? "link")   ; => #t or #f
```
