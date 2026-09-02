---
name: "file/write-lines"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }, { name: lines, type: list }]
returns: "nil"
see_also: ["file/read-lines", "file/write", "file/append"]
---

Write a list of strings to a file, one per line, separated by `\n` (no trailing newline after the last line). Overwrites any existing content.

It is the inverse of `file/read-lines`: `(file/write-lines p (file/read-lines p))` round-trips. Each element should be a string; the lines themselves must not contain `\n`, since that would read back as extra lines.

```sema
(file/write-lines "out.txt" '("a" "b" "c"))   ; writes "a\nb\nc"
(file/read-lines "out.txt")                   ; => ("a" "b" "c")
```
