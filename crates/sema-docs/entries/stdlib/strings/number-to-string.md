---
name: "number/to-string"
module: "strings"
section: "Type Conversions"
params: [{ name: n, type: number }]
returns: "string"
see_also: ["string/to-number", "str", "format"]
---

Convert a number to a string. The inverse is `string->number`.

The Scheme-legacy spelling `number->string` is documented separately under
`math`, where it also covers the optional radix argument this alias never
accepted. Listing it here as an alias registered the same name twice, and
which entry won was nondeterministic — hover showed the radix parameter only
on some runs.

```sema
(number/to-string 42)      ; => "42"
(number/to-string 3.14)    ; => "3.14"
(number/to-string -7)      ; => "-7"
```
