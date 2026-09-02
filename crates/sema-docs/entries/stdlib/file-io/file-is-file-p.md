---
name: "file/is-file?"
module: "file-io"
section: "File Predicates"
params: [{ name: path, type: string }]
returns: "bool"
see_also: ["file/is-directory?", "file/is-symlink?", "file/exists?"]
---

Test if a path is a regular file.

```sema
(file/is-file? "data.txt")   ; => #t
```
