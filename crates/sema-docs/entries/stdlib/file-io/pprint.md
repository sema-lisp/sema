---
name: "pprint"
module: "file-io"
section: "Console I/O"
params: [{ name: x, type: any }]
returns: nil
---

Pretty-print a value to standard output (followed by a newline), wrapping nested structures to roughly 80 columns. Returns `nil`.

```sema
(pprint {:a 1 :b 2})   ; prints {:a 1 :b 2}
```
