---
name: "for-range"
module: "special-forms"
syntax: "(for-range (var start end [step]) body ...)"
---

`for-range` executes `body` repeatedly with `var` bound to successive integers starting at `start` (inclusive) and stopping before `end` (exclusive). An optional fourth element in the binding vector sets the step size, which defaults to `1`. A positive step iterates upward and a negative step iterates downward; a zero step raises an error. The macro expands into a `do` loop with parallel stepping, so every iteration runs in constant stack space thanks to tail-call optimization in the VM. Use `for-range` for counted loops, index-based iteration over arrays, or any situation where you need a numeric counter.

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

Backward iteration uses a negative step:

```sema
(for-range (i 5 0 -1)
  (println i))
;; prints 5 4 3 2 1
```
