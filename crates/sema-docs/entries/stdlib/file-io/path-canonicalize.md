---
name: "path/canonicalize"
module: "file-io"
section: "Path Manipulation"
syntax: "(path/canonicalize path)"
returns: "string"
---

Resolve `path` to an absolute real path, following symlinks and collapsing `.`/`..`. Errors if the path does not exist — that requirement is what makes it the trustworthy form for deciding where a path *actually* points (e.g. before a security check).

```sema
(path/canonicalize "./src/../Cargo.toml")  ; => "/abs/repo/Cargo.toml"
```

For not-yet-created paths, or pure path math that must not touch the filesystem, use `path/relative-to` / `path/within?`.
