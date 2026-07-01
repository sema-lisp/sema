---
name: "path/within?"
module: "file-io"
section: "Path Manipulation"
syntax: "(path/within? base child)"
returns: "boolean"
---

Return `#t` if `child` is contained inside (or equal to) `base`. Resolves `.`/`..` and, for paths that exist, follows symlinks via canonicalization — so it catches both traversal (`../`) and symlink escapes that a substring check would miss. The cornerstone of sandboxing agent file access to a workspace.

```sema
(path/within? "/repo" "/repo/src/main.rs")  ; => #t
(path/within? "/repo" "/repo/../etc")       ; => #f
```
