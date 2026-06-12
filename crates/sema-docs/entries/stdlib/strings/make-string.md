---
name: "make-string"
module: "strings"
section: "Scheme Compatibility Aliases"
params: [{ name: s, type: string }, { name: n, type: int }]
returns: "string"
---

Repeat a string `n` times. In Sema, `make-string` is an alias for `string/repeat`, so it takes a string to repeat and a count (rather than the R7RS `(make-string k char)` signature).

```sema
(make-string "ab" 3)   ; => "ababab"
```
