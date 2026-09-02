---
name: "bytevector?"
module: "predicates"
section: "Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["bytevector", "string?", "vector?", "type-of"]
---

Test if a value is a bytevector.

```sema
(bytevector? #u8())   ; => #t
(bytevector? '())     ; => #f
```
