---
name: "bytevector/copy"
module: "bytevectors"
section: "Copy & Append"
params: [{ name: bv, type: bytevector }, { name: start, type: int, doc: "optional; defaults to 0" }, { name: end, type: int, doc: "optional; defaults to the length" }]
returns: "bytevector"
see_also: ["bytevector-copy", "bytes/slice", "bytevector/append"]
---

Copy a bytevector, optionally restricting to the half-open range `start..end` (`start` defaults to `0`, `end` to the length). The range is half-open, so `end` is excluded. Out-of-bounds ranges signal an error.

```sema
(bytevector/copy #u8(1 2 3 4 5))       ; => #u8(1 2 3 4 5)  (full copy)
(bytevector/copy #u8(1 2 3 4 5) 1 3)   ; => #u8(2 3)        (indices 1..3)
```
