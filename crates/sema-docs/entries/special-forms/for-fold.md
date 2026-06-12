---
name: "for/fold"
module: "special-forms"
syntax: "(for/fold ((acc init) ...) ((var sequence) ...) body)"
---

`for/fold` threads an accumulator through a sequence. On the first iteration each `acc` is bound to its `init` value; on each subsequent iteration `acc` is updated to the result of `body`. When the sequence is exhausted, the final accumulator value (or values, when multiple accumulators are provided) is returned. This form is ideal for aggregating data—summing numbers, building strings, counting matches, or reducing a collection to a single result—without writing an explicit recursive loop.

```sema
(for/fold ((sum 0))
  ((x (range 5)))
  (+ sum x))
;; => 10
```

The accumulator can start as any type. For example, you can build a reversed list by consing each element onto the accumulator:

```sema
(for/fold ((acc '()))
  ((x '(1 2 3 4)))
  (cons x acc))
;; => (4 3 2 1)
```

Multiple accumulator bindings let you compute several aggregated values in a single pass:

```sema
(for/fold ((total 0) (count 0))
  ((x '(1 2 3 4 5)))
  (values (+ total x) (+ count 1)))
;; total => 15, count => 5
```
