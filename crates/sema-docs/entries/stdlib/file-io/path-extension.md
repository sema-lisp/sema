---
name: "path/extension"
module: "file-io"
section: "Path Manipulation"
aliases: ["path/ext"]
---

Return the file extension (without the dot). Returns `""` when the path has no extension.

```sema
(path/extension "file.rs")        ;; => "rs"
(path/extension "file.tar.gz")    ;; => "gz"
(path/extension "Makefile")       ;; => ""
(path/extension ".hidden")        ;; => ""
```

`path/ext` is a legacy alias for `path/extension` — same implementation, same return value.

Previous versions registered `path/dirname`, `path/basename`, and `path/extension` as independent functions that returned `nil` on the no-parent / no-filename / no-extension case. As of the current release, all six names share one implementation per concept and consistently return `""` (matching `path/dir`, `path/filename`, `path/ext`).
