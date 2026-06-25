---
name: "define-record-type"
module: "records"
section: "Defining Record Types"
syntax: "(define-record-type type (constructor field ...) predicate (field accessor) ...)"
---

Define a new record type, generating a constructor, predicate, and one accessor per field.

```sema
(define-record-type point
  (make-point x y)       ; constructor (positional args)
  point?                  ; predicate
  (x point-x)            ; (field-name accessor-name)
  (y point-y))
```

General syntax:

```sema
(define-record-type <type-name>
  (<constructor> <field-name> ...)
  <predicate>
  (<field-name> <accessor>) ...)
```
