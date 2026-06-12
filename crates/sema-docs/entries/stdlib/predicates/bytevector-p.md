---
name: "bytevector?"
module: "predicates"
section: "Type Predicates"
---

Test if a value is a bytevector.

```sema
(bytevector? #u8())   ; => #t
(bytevector? '())     ; => #f
```
