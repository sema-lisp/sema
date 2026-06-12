---
name: "path/filename"
module: "file-io"
section: "Path Manipulation"
aliases: ["path/basename"]
---

Return the filename portion of a path. Returns `""` when there is no filename component (e.g. for `""`).

```sema
(path/filename "/a/b/c.txt")   ;; => "c.txt"
(path/filename "plain.rs")     ;; => "plain.rs"
```

`path/basename` is a legacy alias for `path/filename` — same implementation, same return value.
