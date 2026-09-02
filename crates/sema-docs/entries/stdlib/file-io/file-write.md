---
name: "file/write"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }, { name: content, type: string }]
returns: "nil"
see_also: ["file/read", "file/append", "file/write-lines", "file/write-bytes"]
---

Write a string to a file, overwriting any existing content (creating the file if it does not exist).

This truncates: every call replaces the whole file. To add to a file instead, use `file/append`. Parent directories must already exist — writing to `"a/b/out.txt"` errors if `a/b` is missing, so call `file/mkdir` first.

```sema
(file/write "out.txt" "content")        ; replaces out.txt entirely
(file/write "out.txt" "")               ; truncate to empty
```
