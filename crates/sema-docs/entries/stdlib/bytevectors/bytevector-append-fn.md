---
name: "bytevector-append"
module: "bytevectors"
params: [{ name: bvs, type: bytevector, variadic: true }]
returns: "bytevector"
---

Concatenate any number of bytevectors into a new bytevector. Legacy Scheme name; `bytevector/append` is the namespaced alias.

```sema
(bytevector-append #u8(1 2) #u8(3 4))   ; => #u8(1 2 3 4)
```
