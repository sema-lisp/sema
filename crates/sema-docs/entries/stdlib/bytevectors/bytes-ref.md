---
name: "bytes/ref"
module: "bytevectors"
section: "Byte Ops"
params: [{ name: bv, type: bytevector }, { name: index, type: int, doc: "zero-based" }]
returns: "int"
see_also: ["bytevector/ref", "bytes/slice", "bytes/length"]
---

Return the byte (0–255) at a zero-based index of a bytevector. Out of bounds is an error.

```sema
(bytes/ref (string->utf8 "abc") 1)   ; => 98
```
