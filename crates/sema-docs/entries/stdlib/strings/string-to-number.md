---
name: "string/to-number"
module: "strings"
section: "Type Conversions"
aliases: ["string->number"]
params: [{ name: s, type: string }]
returns: "number"
see_also: ["string/number?", "string->float", "number/to-string"]
---

Parse a string as a number, using the same rules as the reader: integers, floats, exponents (`1e3`), rationals (`1/2`), bignums, and an optional leading sign. Surrounding whitespace is ignored. Returns `#f` when the text is not a number.

Use `string/number?` to test parseability without raising, and `string->float` when you always want a float (even for integer text).

```sema
(string/to-number "42")     ; => 42
(string/to-number "3.14")   ; => 3.14
(string/to-number "1e3")    ; => 1000.0
(string/to-number "  42  ")  ; => 42
(string/to-number "+7")      ; => 7
(string/to-number "abc")     ; => #f
```
