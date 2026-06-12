---
name: "read-many"
module: "file-io"
section: "Reader"
params: [{ name: s, type: string }]
returns: list
---

Parse a string containing zero or more Sema expressions and return them as a list of data values (unevaluated). Alias: `io/read-many`.

```sema
(read-many "1 2 3")   ; => (1 2 3)
```
