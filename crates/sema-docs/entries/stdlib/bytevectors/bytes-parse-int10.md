---
name: "bytes/parse-int10"
module: "bytevectors"
section: "Byte Ops"
params: [{ name: bv, type: bytevector }, { name: start, type: int, optional: true }, { name: end, type: int, doc: "exclusive; defaults to the length", optional: true }]
returns: "int"
see_also: ["bytes/slice", "bytes/->string", "bytes/find"]
---

Parse ASCII `-?digits(.digit)?` as a base-10 integer scaled by 10: `"-12.3"` → `-123`, `"5"` → `50`. This is the fixed-point trick for one-decimal measurements (e.g. 1BRC temperatures): the value times ten as an exact int, with no float math or string allocation. At most one fractional digit is accepted; anything else (empty input, stray characters, more decimals) is an error. The optional `start`/`end` range parses a sub-slice in place.

```sema
(bytes/parse-int10 (string->utf8 "-12.3"))            ; => -123
(bytes/parse-int10 (string->utf8 "5"))                ; => 50
(bytes/parse-int10 (string->utf8 "Oslo;-12.3") 5)     ; => -123
```
