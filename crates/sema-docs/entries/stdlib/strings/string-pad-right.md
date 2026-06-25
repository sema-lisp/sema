---
name: "string/pad-right"
module: "strings"
section: "Core String Operations"
syntax: "(string/pad-right s width [pad])"
returns: "string"
---

Pad a string on the right to a given width.

```sema
(string/pad-right "hi" 5)       ; => "hi   "
(string/pad-right "42" 5 "0")   ; => "42000"
```
