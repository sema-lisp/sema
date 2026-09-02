---
name: "path/absolute"
module: "file-io"
section: "Path Manipulation"
params: [{ name: path, type: string }]
returns: "string"
see_also: ["path/canonicalize", "path/absolute?", "path/join"]
---

Return the absolute path.

```sema
(path/absolute ".")   ; => "/full/path/to/current/dir"
```
