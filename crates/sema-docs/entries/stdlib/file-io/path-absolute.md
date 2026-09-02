---
name: "path/absolute"
module: "file-io"
section: "Path Manipulation"
params: [{ name: path, type: string }]
returns: "string"
see_also: ["path/canonicalize", "path/absolute?", "path/join"]
---

Resolve `path` against the current working directory and return the
canonical absolute form: symlinks are followed and `.`/`..` segments are
removed. The path **must exist**; a missing file or directory raises an I/O
error. This is the same resolution as `path/canonicalize`.

For a purely textual absolute path that does not touch the filesystem, join
the working directory yourself: `(path/join (sys/cwd) path)`. Use
`path/absolute?` to test whether a string already starts at the root.

Requires the `fs-read` capability and, under `--allowed-paths`, a path inside
an allowed directory.

```sema
(path/absolute ".")             ; => "/full/path/to/current/dir"
(path/absolute "src/../src")    ; => ".../src" (the `..` is resolved)
(path/absolute "does-not-exist")   ; => error: I/O error: path/absolute does-not-exist: No such file or directory
(path/absolute? "/etc/hosts")   ; => #t
```
