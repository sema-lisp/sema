---
name: "number->string"
module: "math"
section: "Number/String Conversion"
params: [{ name: n, type: number }, { name: radix, type: integer, doc: "Optional radix (2, 8, 10, or 16); default 10" }]
returns: "string"
see_also: ["string->number"]
---

Convert a number to its string representation. With the default radix 10 it renders any number in the tower — integers, rationals (`1/3`), floats, and complex numbers. An optional radix of 2, 8, 10, or 16 selects the output base, but only *exact integers* can be printed in a non-decimal base; passing a float, rational, or complex with a non-10 radix raises a type error (`expected exact integer`).

```sema
(number->string 42)       ; => "42"
(number->string 1/3)      ; => "1/3"
(number->string 3.14)     ; => "3.14"
(number->string 3+4i)     ; => "3+4i"
(number->string 255 16)   ; => "ff"
(number->string 5 2)      ; => "101"
(number->string 42 8)     ; => "52"
```

[`string->number`](../string-to-number) is the inverse and round-trips with the
same radix.
