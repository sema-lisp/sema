---
name: "for/filter"
module: "special-forms"
syntax: "(for/filter ((var sequence) ...) body)"
---

`for/filter` is a list-comprehension form that iterates over a sequence, binds each element to `var`, and evaluates `body` as a predicate. It returns a list containing every element for which the predicate is truthy, preserving the original order. When multiple binding pairs are provided, the sequences are traversed in parallel and the loop stops when the shortest sequence is exhausted. This form is convenient when you need to extract a subset of a collection without writing an explicit `filter` pipeline.

```sema
(for/filter ((x (range 10)))
  (even? x))
;; => (0 2 4 6 8)
```

You can use any expression as the predicate, including calls to user-defined functions:

```sema
(define (positive? n) (> n 0))
(for/filter ((x '(-3 -1 0 2 4)))
  (positive? x))
;; => (2 4)
```

If no element satisfies the predicate, `for/filter` returns the empty list.

```sema
(for/filter ((x '(1 3 5)))
  (even? x))
;; => ()
```
