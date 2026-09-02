---
name: "zip/create"
module: "archive"
section: "Compression & Archives"
params: [{ name: out-path, type: string }, { name: files, type: list }]
returns: "int"
see_also: ["zip/extract", "zip/list", "tar/create"]
---

Create a ZIP archive at `out-path` containing the given list of files. Each file is added under its basename using deflate compression. Returns the number of entries written. Requires the `fs-write` capability.

```sema
(zip/create "bundle.zip" (list "a.txt" "b.txt"))
```
