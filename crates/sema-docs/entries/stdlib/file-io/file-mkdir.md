---
name: "file/mkdir"
module: "file-io"
section: "Directory Operations"
params: [{ name: path, type: string }]
returns: "nil"
see_also: ["file/is-directory?", "file/exists?", "file/write"]
---

Create a directory, including any missing parent directories (like `mkdir -p`).

`(file/mkdir "a/b/c")` creates `a`, `a/b`, and `a/b/c` in one call. Useful before `file/write`, which does not create parent directories on its own.

```sema
(file/mkdir "logs/2026/06")   ; creates the whole chain
```
