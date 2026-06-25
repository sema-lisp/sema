---
name: "file/read"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }]
returns: "string"
---

Read the entire contents of a file as a string.

```sema
(file/read "data.txt")   ; => "file contents..."
```
