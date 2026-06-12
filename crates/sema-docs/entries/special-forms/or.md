---
name: "or"
module: "special-forms"
syntax: "(or expr ...)"
---

Short-circuit logical OR. Evaluates expressions from left to right and returns the first truthy value it encounters. If all expressions are falsy, it returns the value of the last expression. With no arguments, `or` returns `#f`.

Because it is a special form, `or` stops evaluating as soon as a truthy result is found; subsequent expressions are never touched. This is useful for providing default values, trying alternatives, or fallbacks. Only `nil` and `#f` are falsy in Sema.

```sema
(or #f #t)            ; => #t
(or #f #f)            ; => #f
```

```sema
(define config nil)
(or config {})        ; => {} (default when config is nil)
```

```sema
(define port (or (:port env) 8080))
port                  ; => 8080 if env has no :port key
```

```sema
(or)                  ; => #f
(or 42 (crash))       ; => 42 (crash is never called)
```
