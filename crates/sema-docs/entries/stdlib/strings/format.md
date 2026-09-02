---
name: "format"
module: "strings"
section: "Scheme Compatibility Aliases"
syntax: "(format fmt arg ...)"
returns: "string"
see_also: ["str", "format/form", "number/to-string"]
---

Format a string, substituting each `~a` placeholder with the next argument's display form (the same plain rendering `str` produces). Use `~s` instead when you want the *readable* form — strings come back quoted and escaped.

`~a` (aesthetic) is for human-facing text; `~s` (s-expression) round-trips back through the reader, so it's what you want when emitting code or debug output.

```sema
(format "~a is ~a" "Sema" "great")   ; => "Sema is great"
(format "~a + ~a = ~a" 1 2 3)        ; => "1 + 2 = 3"

;; ~a shows the raw value, ~s shows the readable (quoted) form
(format "~a vs ~s" "hi" "hi")        ; => "hi vs \"hi\""
```

For single-value conversion reach for `str`; for inline interpolation, f-strings (`f"${name} is great"`) are usually clearer than `format`.
