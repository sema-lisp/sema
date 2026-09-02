---
name: "record?"
module: "predicates"
section: "Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["define-record-type", "type-of", "map?"]
---

Test if a value is a record instance (any value built by a `define-record-type` constructor). Use `type-of` to recover the specific record type tag.

```sema
(define-record-type point
  (make-point x y)
  point?
  (x point-x)
  (y point-y))

(record? (make-point 1 2))   ; => #t
(record? 42)                 ; => #f
(type-of (make-point 1 2))   ; => :point
```

Each record type also generates its own predicate (`point?` above); `record?` is the generic test that holds for any record regardless of type.
