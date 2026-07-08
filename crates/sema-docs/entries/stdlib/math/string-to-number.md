---
name: "string->number"
module: "math"
section: "Number/String Conversion"
params: [{ name: s, type: string }, { name: radix, type: integer, doc: "Optional radix (2, 8, 10, or 16); default 10" }]
returns: "number or #f"
see_also: ["number->string"]
---

Parse a string as a number, returning `#f` (never an error) when the string is not a valid number. With the default radix 10 it accepts the whole tower: integers, rationals (`1/3`), floats (`3.14`, `1e3`), and complex literals (`3+4i`). Pass a radix of 2, 8, or 16 to parse the string as an integer in that base.

```sema
(string->number "42")     ; => 42
(string->number "1/3")    ; => 1/3
(string->number "3.14")   ; => 3.14
(string->number "3+4i")   ; => 3+4i
(string->number "ff" 16)  ; => 255
(string->number "101" 2)  ; => 5
(string->number "nope")   ; => #f
(string->number "")       ; => #f
```

Returning `#f` instead of raising makes it convenient in a conditional. The
inverse is [`number->string`](../number-to-string), and they round-trip with a
matching radix.
