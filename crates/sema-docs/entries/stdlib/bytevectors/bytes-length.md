---
name: "bytes/length"
module: "bytevectors"
section: "Byte Ops"
params: [{ name: bv, type: bytevector }]
returns: "int"
see_also: ["bytevector/length", "bytes/slice", "bytes/ref"]
---

Return the length of a bytevector in bytes. Part of the `bytes/*` family for byte-oriented hot loops (no UTF-8 work); same result as `bytevector/length`.

```sema
(bytes/length (string->utf8 "abc"))   ; => 3
```
