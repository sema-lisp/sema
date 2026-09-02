---
name: "string-ci=?"
module: "strings"
section: "Unicode & Encoding"
params: [{ name: a, type: string }, { name: b, type: string }]
returns: "bool"
see_also: ["string/foldcase", "char-ci=?", "string/lower"]
---

Case-insensitive string equality comparison. Compares two strings after applying case folding to both.

```sema
(string-ci=? "Hello" "hello")   ; => #t
(string-ci=? "ABC" "abc")       ; => #t
(string-ci=? "CAFÉ" "café")     ; => #t
(string-ci=? "hello" "world")   ; => #f
```
