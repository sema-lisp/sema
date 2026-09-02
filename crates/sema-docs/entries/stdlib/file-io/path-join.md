---
name: "path/join"
module: "file-io"
section: "Path Manipulation"
syntax: "(path/join part ...)"
returns: "string"
see_also: ["path/dir", "path/filename", "path/relative-to"]
---

Join two or more path components with the platform separator, collapsing redundant slashes. Prefer this over string concatenation so you do not double or miss separators.

Gotcha: an absolute component resets the path — `(path/join "a" "/b" "c")` is `"/b/c"`, not `"a/b/c"` (standard `Path::join` semantics).

```sema
(path/join "src" "main.rs")    ; => "src/main.rs"
(path/join "a" "b" "c.txt")    ; => "a/b/c.txt"
(path/join "a/" "b")           ; => "a/b"    (no doubled slash)
(path/join "a" "/b" "c")       ; => "/b/c"   (absolute resets)
```
