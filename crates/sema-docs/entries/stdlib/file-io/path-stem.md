---
name: "path/stem"
module: "file-io"
section: "Path Manipulation"
params: [{ name: path, type: string }]
returns: "string"
see_also: ["path/extension", "path/filename"]
---

Return the filename without its final extension. Only the last extension is removed, so `stem` and `extension` complement each other: `"archive.tar.gz"` splits into stem `"archive.tar"` and extension `"gz"`. Operates on the filename only — any directory prefix is dropped first.

```sema
(path/stem "file.rs")          ; => "file"
(path/stem "archive.tar.gz")   ; => "archive.tar"   (only ".gz" stripped)
(path/stem "/a/b/notes.md")    ; => "notes"          (dir dropped too)
```
