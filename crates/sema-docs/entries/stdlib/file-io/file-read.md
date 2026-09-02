---
name: "file/read"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }]
returns: "string"
see_also: ["file/read-bytes", "file/read-lines", "file/write"]
---

Read the entire contents of a file as a string. Errors if the file is missing or is not valid UTF-8 — guard with `file/exists?`, or use `file/read-bytes` for binary data.

For large files you want to process line by line, prefer `file/for-each-line` / `file/fold-lines` over reading the whole thing into one string.

```sema
(file/read "data.txt")   ; => "file contents..."
(when (file/exists? "data.txt")
  (println (file/read "data.txt")))
```
