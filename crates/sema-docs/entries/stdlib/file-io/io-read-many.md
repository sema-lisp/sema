---
name: "io/read-many"
module: "file-io"
section: "Console I/O"
params: [{ name: source, type: string }]
returns: "list"
---

Parse a string of Sema source into a list of every top-level datum it contains (alias of `read-many`). Unlike `io/read-line`, this does not touch stdin — it runs the reader over the given string and returns the parsed S-expressions.

```sema
(io/read-many "(+ 1 2) (* 3 4)")  ; => ((+ 1 2) (* 3 4))
```
