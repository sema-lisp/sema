---
name: "file/is-directory?"
module: "file-io"
section: "File Predicates"
params: [{ name: path, type: string }]
returns: "bool"
see_also: ["file/is-file?", "file/is-symlink?", "file/exists?"]
---

Test if a path is a directory.

```sema
(file/is-directory? "src/")   ; => #t
```
