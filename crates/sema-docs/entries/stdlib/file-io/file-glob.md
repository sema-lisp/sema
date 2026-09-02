---
name: "file/glob"
module: "file-io"
section: "Directory Operations"
params: [{ name: pattern, type: string }]
returns: "list"
see_also: ["file/list", "file/is-file?", "file/info"]
---

Find files matching a glob pattern, returning their paths. `*` matches within one path segment, `?` matches a single character, and `**` matches across directories (recursive).

The results are full paths in the form the pattern produced (so a relative pattern yields relative paths), unlike `file/list`, which returns the bare entry names of a single directory without descending. Reach for `file/glob` when you want to walk a tree by extension; reach for `file/list` for a flat directory listing.

```sema
(file/glob "src/**/*.rs")   ; => ("src/main.rs" "src/vm/lower.rs" ...)  (recursive)
(file/glob "*.txt")         ; => ("readme.txt" "notes.txt")             (this dir only)
```
