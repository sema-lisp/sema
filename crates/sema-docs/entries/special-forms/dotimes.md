---
name: "dotimes"
module: "special-forms"
syntax: "(dotimes (var count) body ...)"
---

`dotimes` evaluates `body` exactly `count` times, binding `var` to the integers `0` through `count - 1` on successive iterations. It is implemented as a prelude macro that expands into a `do` loop, so it executes in constant stack space and benefits from tail-call optimization. The return value is `nil` because the generated loop has no result expression after its termination test. `dotimes` is the idiomatic choice for side-effecting loops that only need a counter, such as repeating an action, printing output, or indexing into a fixed-size structure.

```sema
(dotimes (i 3)
  (println i))
;; prints 0, 1, 2
```

If you need to accumulate a result, mutate a variable in the enclosing scope:

```sema
(let ((total 0))
  (dotimes (i 4)
    (set! total (+ total i)))
  total)
;; => 6
```

When `count` is zero or negative, the body never executes.
