---
name: "integer?"
module: "predicates"
section: "Numeric Predicates"
---

Test if a value is an integer. This is R7RS *value-based*: any number with no fractional part is an integer regardless of representation — an integer-valued float like `3.0` counts, and so do bignums (arbitrary-precision integers beyond machine-word range).

```sema
(integer? 42)     ; => #t
(integer? 3.14)   ; => #f
(integer? 3.0)    ; => #t   (integer-valued float — checked by value, not type)
(integer? "42")   ; => #f
```

Use `number?` to accept any numeric type; use `float?` to check specifically for the float representation.
