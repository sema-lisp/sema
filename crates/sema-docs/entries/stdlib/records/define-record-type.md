---
name: "define-record-type"
module: "records"
section: "Defining Record Types"
syntax: "(define-record-type type (constructor field ...) predicate (field accessor) ...)"
returns: "nil"
see_also: ["record?", "type-of", "type"]
---

Define a new record type, generating a constructor, predicate, and one accessor per field.

```sema
(define-record-type point
  (make-point x y)       ; constructor (positional args)
  point?                  ; predicate
  (x point-x)            ; (field-name accessor-name)
  (y point-y))
```

Each of the three generated names is a normal binding you can call:

```sema
(define-record-type point
  (make-point x y) point?
  (x point-x) (y point-y))

(define p (make-point 3 4))
(point-x p)      ; => 3
(point? p)       ; => #t
(point? 42)      ; => #f
(type p)         ; => :point
```

Records are **immutable** — there are no generated field setters, so "updating" means constructing a new record (`(make-point (point-x p) 10)`). Unlike a plain map, the predicate gives you a real nominal type: two records with the same fields but different type names are distinguishable, and `point?` only answers `#t` for points.

General syntax:

```sema
(define-record-type <type-name>
  (<constructor> <field-name> ...)
  <predicate>
  (<field-name> <accessor>) ...)
```
