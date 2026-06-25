---
name: "file/write"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }, { name: content, type: string }]
returns: "nil"
---

Write a string to a file, overwriting any existing content.

```sema
(file/write "out.txt" "content")
```
