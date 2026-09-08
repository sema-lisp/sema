---
name: "string/to-number"
module: "strings"
section: "Type Conversions"
aliases: ["string->number"]
params: [{ name: s, type: string }, { name: radix, type: int, optional: true, doc: "2, 8, 10, or 16" }]
returns: "number"
see_also: ["string/number?", "string->float", "number/to-string"]
---

Parse a string as a number. With no radix, it uses the reader's rules for integers, floats, exponents (`1e3`), rationals (`1/2`), and bignums. The optional radix accepts 2, 8, 10, or 16 for integer input. Surrounding whitespace is ignored. Returns `#f` when the text is not a number.

Use `string/number?` to test parseability without raising, and `string->float` when you always want a float (even for integer text).

```sema
(string/to-number "42")     ; => 42
(string/to-number "3.14")   ; => 3.14
(string/to-number "1e3")    ; => 1000.0
(string/to-number "  42  ")  ; => 42
(string/to-number "+7")      ; => 7
(string/to-number "ff" 16)   ; => 255
(string/to-number "abc")     ; => #f
```
