---
name: "do"
module: "special-forms"
syntax: "(do ((var init [step]) ...) (test [result ...]) body ...)"
---

Scheme-style iterative loop with explicit variable bindings, step expressions, and a termination test. Each binding specifies a variable name, an initial value, and an optional step expression. On each iteration, all variables are updated in parallel using their step expressions (or retain their current value if no step is given). The loop terminates when the test expression evaluates to truthy, at which point the result expressions are evaluated and the last result is returned. If no result expressions are provided, the loop returns `nil`.

The body expressions, if any, are evaluated on every iteration before the step update and are typically used for side effects. `do` is useful for numeric iteration, accumulation, and any loop that requires parallel variable updates. For sequential binding semantics, use `let*` inside the loop body instead.

```sema
(do ((i 0 (+ i 1))
     (sum 0 (+ sum i)))
    ((= i 10) sum))
;; => 45
```

```sema
(do ((i 0 (+ i 1)))
    ((= i 5))
  (println i))
;; prints 0 through 4, returns nil
```

```sema
(do ((n 10 (/ n 2)))
    ((= n 0) "done")
  (println n))
;; prints 10, 5, 2, 1
```

**Note:** Both the tree-walker and the VM support `do`. The VM compiles it to a dedicated `DoLoop` IR node with parallel step assignment.