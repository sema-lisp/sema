---
name: "and"
module: "special-forms"
syntax: "(and expr ...)"
---

Short-circuit logical AND. Evaluates expressions from left to right and returns the first falsy value it encounters. If all expressions are truthy, it returns the value of the last expression. With no arguments, `and` returns `#t`.

Because it is a special form, `and` stops evaluating as soon as a falsy result is found; subsequent expressions are never touched. This makes it ideal for chained predicates, guarded property access, or pipelines where later steps depend on earlier ones succeeding. Only `nil` and `#f` are falsy in Sema.

```sema
(and #t #t)           ; => #t
(and #t #f)           ; => #f
```

```sema
(and "hello" 42 #t)   ; => #t (all truthy, last value returned)
```

```sema
(define m {:a 1})
(and m (:a m) (+ (:a m) 10))   ; => 11
```

```sema
(and)                 ; => #t
(and #f (expensive))  ; => #f (expensive is never called)
```
