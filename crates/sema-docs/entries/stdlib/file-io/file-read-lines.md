---
name: "file/read-lines"
module: "file-io"
section: "File Operations"
---

Read a file as a list of lines. Handles both `\n` and `\r\n` line endings. An empty file returns an empty list.

```sema
(file/read-lines "data.txt")   ; => ("line 1" "line 2" "line 3")
(file/read-lines "empty.txt")  ; => ()
```
