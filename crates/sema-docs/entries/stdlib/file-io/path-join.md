---
name: "path/join"
module: "file-io"
section: "Path Manipulation"
---

Join path components.

```sema
(path/join "src" "main.rs")   ; => "src/main.rs"
(path/join "a" "b" "c.txt")  ; => "a/b/c.txt"
```
