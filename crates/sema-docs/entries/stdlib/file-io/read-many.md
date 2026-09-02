---
name: "read-many"
module: "file-io"
section: "Reader"
params: [{ name: s, type: string }]
returns: list
see_also: ["read", "read-line", "io/read-many"]
---

Parse a string containing zero or more Sema expressions and return them as a list of data values (unevaluated). Alias: `io/read-many`.

Unlike `read` (which returns just the first datum), `read-many` consumes the whole string, so it is the right tool for reading a file of multiple forms. An empty (or whitespace-only) string yields the empty list.

```sema
(read-many "1 2 3")            ; => (1 2 3)
(read-many "(+ 1 2) (* 3 4)")  ; => ((+ 1 2) (* 3 4))
(read-many "")                 ; => ()
```
