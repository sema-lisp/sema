---
name: "time/format"
module: "datetime"
section: "Formatting"
params: [{ name: timestamp, type: number, doc: "UTC Unix timestamp" }, { name: format-string, type: string, doc: "strftime-style format" }]
returns: "string"
---

Format a UTC Unix timestamp using a [strftime](#strftime-format-directives)-style format string.

```sema
(time/format timestamp format-string) ; => string
```

```sema
(define ts 1736943000.0)  ; 2025-01-15 12:10:00 UTC

(time/format ts "%Y-%m-%d")            ; => "2025-01-15"
(time/format ts "%H:%M:%S")            ; => "12:10:00"
(time/format ts "%Y-%m-%d %H:%M:%S")   ; => "2025-01-15 12:10:00"
(time/format ts "%A, %B %d, %Y")       ; => "Wednesday, January 15, 2025"
(time/format ts "%F")                  ; => "2025-01-15"  (shorthand for %Y-%m-%d)
(time/format ts "%T")                  ; => "12:10:00"    (shorthand for %H:%M:%S)
```
