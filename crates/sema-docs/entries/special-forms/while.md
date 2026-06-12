---
name: "while"
module: "special-forms"
syntax: "(while condition body ...)"
---

Repeatedly evaluate the body expressions while the condition is truthy. The condition is tested before each iteration. If the condition is initially falsy, the body is never executed. `while` always returns `nil`. Loop variables must be mutated explicitly with `set!` or updated through mutable data structures.

`while` is the simplest imperative looping construct in Sema. It is well-suited for loops where the termination condition depends on external state or complex logic that is awkward to express with `do` or named `let`. Because the condition and body are re-evaluated on each iteration, be careful to ensure the loop makes progress to avoid infinite loops.

```sema
(define i 0)
(while (< i 5)
  (println i)
  (set! i (+ i 1)))
;; prints 0 through 4
```

```sema
(let ((n 100))
  (while (> n 1)
    (println n)
    (set! n (if (even? n) (/ n 2) (+ (* n 3) 1)))))
;; Collatz sequence
```

```sema
(let ((found #f)
      (i 0))
  (while (and (not found) (< i 100))
    (when (= (nth items i) target)
      (set! found #t))
    (set! i (+ i 1)))
  found)
```

**Note:** `while` is supported by both the tree-walker and the VM. In the VM it compiles to a conditional jump loop.