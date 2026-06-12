---
name: "->>"
module: "special-forms"
syntax: "(->> val form ...)"
---

`->>` is the thread-last macro. It inserts `val` as the last argument of each successive `form`, creating a pipeline that flows through sequence-processing and higher-order functions. A bare symbol `f` is treated as `(f)`. Like `->`, it is a recursive prelude macro that expands into ordinary nested calls before evaluation or compilation, incurring no runtime cost. Thread-last is idiomatic for list comprehensions and collection pipelines where the collection is conventionally the final argument, such as with `map`, `filter`, `fold`, and `take`.

```sema
(->> (range 1 100)
     (filter even?)
     (map (fn (x) (* x x)))
     (take 5))
;; => (4 16 36 64 100)
```

Thread-last composes naturally with functions that accept callbacks or options before the primary collection:

```sema
(->> '(1 2 3 4 5)
     (filter (fn (x) (> x 2)))
     (map (fn (x) (* x 10))))
;; => (30 40 50)
```

When a single step needs the value in a different position, interleave `as->` or switch to `->` for that step.

```sema
(->> '(1 2 3)
     (map (fn (x) (* x x)))
     (as-> nums (cons 0 nums))
     (reverse))
;; => (9 4 1 0)
```

**Note:** `->>` is a prelude macro and requires no import.
