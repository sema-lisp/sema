---
name: "values"
module: "lists"
section: "Multiple Values"
syntax: "(values expr ...)"
returns: "any"
---

Produce zero or more values from a single expression, for use with `call-with-values`, `let-values`, `let*-values`, or `define-values`. This is R7RS's mechanism for a producer to return more than one value without packing them into a list.

`(values x)` — exactly one value — is identity: it returns `x` unchanged, so a single-value `values` call flows through ordinary single-value contexts like arithmetic or comparison exactly as if `values` weren't there.

```sema
(+ (values 5) 1)          ; => 6
(= (values 5) 5)          ; => #t
```

Zero or two-or-more values only make sense when spread by one of the values-consuming forms:

```sema
(call-with-values (lambda () (values 1 2 3)) +)   ; => 6
(let-values (((a b) (values 1 2))) (+ a b))       ; => 3
```

Passing a multiple-values result to an ordinary function that isn't expecting one (i.e. not through `call-with-values`/`let-values`/`define-values`) is unspecified by R7RS; Sema currently represents it as an opaque record, so it won't silently spread into that function's arguments.

**See also:** `call-with-values` (call a producer, spread its values into a consumer), `let-values`/`let*-values` (bind multiple values as local variables), `define-values` (bind multiple values as top-level/global definitions).
