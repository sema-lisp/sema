---
name: "string/foldcase"
module: "strings"
section: "Unicode & Encoding"
---

Apply Unicode case folding to a string. Useful for case-insensitive comparisons and normalization. Uses full Unicode-aware lowercasing.

```sema
(string/foldcase "HELLO")        ; => "hello"
(string/foldcase "Hello World")  ; => "hello world"
(string/foldcase "Straße")       ; => "straße"
(string/foldcase "ΩΜΕΓΑ")        ; => "ωμεγα"
```
