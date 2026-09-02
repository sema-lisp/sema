---
name: "file/exists?"
module: "file-io"
section: "File Predicates"
params: [{ name: path, type: string }]
returns: "bool"
see_also: ["file/is-file?", "file/is-directory?", "file/info"]
---

Test whether a path exists, whether it is a file, directory, or symlink. Use `file/is-file?` or `file/is-directory?` when you need to know *which*.

```sema
(file/exists? "data.txt")   ; => #t or #f
```
