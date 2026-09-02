---
name: "file/delete"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }]
returns: "nil"
see_also: ["file/exists?", "file/rename", "file/copy"]
---

Delete a file. Errors if the path does not exist, so guard with `file/exists?` when the file may be absent.

```sema
(when (file/exists? "tmp.txt")
  (file/delete "tmp.txt"))
```
