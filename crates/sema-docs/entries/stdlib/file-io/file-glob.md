---
name: "file/glob"
module: "file-io"
section: "Directory Operations"
---

Find files matching a glob pattern.

```sema
(file/glob "src/**/*.rs")      ; => ("src/main.rs" "src/lib.rs" ...)
(file/glob "*.txt")            ; => ("readme.txt" "notes.txt")
```
