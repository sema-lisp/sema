---
name: "record?"
module: "records"
section: "Introspection"
---

Test if a value is any record instance (of any record type).

```sema
(record? (make-point 3 4))   ; => #t
(record? {:x 3 :y 4})        ; => #f
(record? 42)                 ; => #f
```
