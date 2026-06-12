---
name: "for-range"
module: "special-forms"
syntax: "(for-range (var start end [step]) body ...)"
---

`for-range` executes `body` repeatedly with `var` bound to successive integers starting at `start` (inclusive) and continuing while strictly less than `end` (exclusive). An optional fourth element in the binding vector sets the step size, which defaults to `1`. The macro expands into a `do` loop with parallel stepping, so every iteration runs in constant stack space thanks to tail-call optimization in both the tree-walker and VM backends. Use `for-range` for counted loops, index-based iteration over arrays, or any situation where you need a numeric counter.

```sema
(for-range (i 0 5)
  (println i))
;; prints 0 1 2 3 4
```

You can specify a custom step to skip elements:

```sema
(for-range (i 0 10 2)
  (println i))
;; prints 0 2 4 6 8
```

**Note:** The step must evaluate to a positive number. `for-range` uses a single `>=` termination test, so backward iteration (negative step) is not supported by this macro; use a named `let` or `do` loop for decrementing ranges.
