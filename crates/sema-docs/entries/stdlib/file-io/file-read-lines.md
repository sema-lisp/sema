---
name: "file/read-lines"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }]
returns: "list"
see_also: ["file/read", "file/write-lines", "file/for-each-line"]
---

Read a file as a list of lines. Handles both `\n` and `\r\n` line endings. An empty file returns an empty list.

Loads the entire file into memory at once, which is fine for small-to-medium files. For files too large to hold in RAM, stream them instead with `file/for-each-line` (side effects) or `file/fold-lines` (running accumulator).

```sema
(file/read-lines "data.txt")   ; => ("line 1" "line 2" "line 3")
(file/read-lines "empty.txt")  ; => ()
```
