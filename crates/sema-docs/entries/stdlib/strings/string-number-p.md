---
name: "string/number?"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "bool"
see_also: ["string/to-number", "string->float", "number?"]
---

Test if a string represents a valid number, using the same rules as `string/to-number` (which returns `#f` on bad input). Surrounding whitespace is ignored.

```sema
(string/number? "42")      ; => #t
(string/number? "3.14")    ; => #t
(string/number? "1e3")     ; => #t
(string/number? "-3.5")    ; => #t
(string/number? "hello")   ; => #f
(string/number? "  42 ")   ; => #t  ; surrounding whitespace is ignored
(string/number? "")        ; => #f
```
