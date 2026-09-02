---
name: "list->string"
module: "strings"
section: "Type Conversions"
params: [{ name: chars, type: list }]
returns: "string"
see_also: ["string/chars", "string/to-list", "string/map"]
---

Convert a list of characters back into a string. The inverse of `string/chars`, so the two round-trip — useful for character-level transforms via `map`/`filter`.

```sema
(list->string '(#\h #\i))   ; => "hi"

;; Round-trip with an uppercase transform
(list->string (map char/upcase (string/chars "abc")))  ; => "ABC"
```
