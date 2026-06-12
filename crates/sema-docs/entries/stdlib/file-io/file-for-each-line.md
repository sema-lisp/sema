---
name: "file/for-each-line"
module: "file-io"
section: "File Operations"
---

Iterate over lines of a file, calling a function on each line. Memory-efficient for large files.

```sema
(file/for-each-line "data.txt"
  (fn (line) (println line)))
```
