---
name: "path/dir"
module: "file-io"
section: "Path Manipulation"
aliases: ["path/dirname"]
params: [{ name: path, type: string }]
returns: "string"
see_also: ["path/filename", "path/join", "path/stem"]
---

Return the directory portion of a path. Returns `""` when the path has no parent component.

```sema
(path/dir "/a/b/c.txt")   ;; => "/a/b"
(path/dir "foo")          ;; => ""
```

`path/dirname` is a legacy alias for `path/dir` — same implementation, same return value.
