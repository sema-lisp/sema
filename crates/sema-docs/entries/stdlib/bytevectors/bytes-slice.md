---
name: "bytes/slice"
module: "bytevectors"
section: "Byte Ops"
params: [{ name: bv, type: bytevector }, { name: start, type: int }, { name: end, type: int, doc: "exclusive; defaults to the length", optional: true }]
returns: "bytevector"
see_also: ["bytevector/copy", "bytes/find", "bytes/ref"]
---

Copy the byte range `start..end` (end exclusive, defaulting to the end) out of a bytevector. Indices are byte offsets — no UTF-8 validation or char-boundary rules, unlike `substring`. Prefer the optional start/end arguments of `bytes/->string`, `bytes/find`, and `bytes/parse-int10` in hot loops; they read the same range without this copy.

```sema
(bytes/slice (string->utf8 "hello") 1 3)   ; => #u8(101 108)
(bytes/->string (bytes/slice (string->utf8 "hello") 3))   ; => "lo"
```
