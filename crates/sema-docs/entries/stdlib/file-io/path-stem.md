---
name: "path/stem"
module: "file-io"
section: "Path Manipulation"
---

Return the filename without extension.

```sema
(path/stem "file.rs")      ; => "file"
(path/stem "archive.tar.gz")  ; => "archive.tar"
```
