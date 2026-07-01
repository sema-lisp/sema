---
name: "path/relative-to"
module: "file-io"
section: "Path Manipulation"
syntax: "(path/relative-to base path)"
returns: "string"
---

Express `path` relative to `base` using pure path math (no filesystem access, so it works for paths that do not exist). Inserts `..` segments when `path` is outside `base`.

```sema
(path/relative-to "/a/b" "/a/b/c/d")  ; => "c/d"
(path/relative-to "/a/b/c" "/a/x")    ; => "../../x"
(path/relative-to "/a/b" "/a/b")      ; => "."
```
