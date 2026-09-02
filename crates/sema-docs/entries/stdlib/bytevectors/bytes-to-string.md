---
name: "bytes/->string"
module: "bytevectors"
section: "Byte Ops"
params: [{ name: bv, type: bytevector }, { name: start, type: int, optional: true }, { name: end, type: int, doc: "exclusive; defaults to the length", optional: true }]
returns: "string"
see_also: ["utf8/to-string", "bytes/slice", "string/to-utf8"]
---

Decode a bytevector (or the byte range `start..end` of it) as a UTF-8 string. Invalid UTF-8 is an error, like `utf8->string`; the optional range decodes a sub-slice with no intermediate `bytes/slice` allocation.

```sema
(bytes/->string (string->utf8 "Oslo;-12.3") 0 4)   ; => "Oslo"
(bytes/->string (string->utf8 "abc"))              ; => "abc"
```
