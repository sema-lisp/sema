---
name: "->"
module: "special-forms"
syntax: "(-> val form ...)"
---

`->` is the thread-first macro. It inserts `val` as the first argument of each successive `form`, creating a left-to-right pipeline. A bare symbol `f` is treated as `(f)`. The macro expands recursively at macro-expansion time into ordinary nested calls, so there is no runtime overhead. Thread-first is especially useful for data transformations, building nested structures, and drilling into maps or records, because most accessor and update functions take the data as their primary argument.

```sema
(-> 5 (+ 3) (* 2))
;; => 16
```

You can mix function calls and bare symbols. The following example drills into nested map keys after decoding JSON:

```sema
(-> response :body json/decode :data :users)
;; equivalent to (:users (:data (json/decode (:body response))))
```

Because the threaded value always appears first, `->` pairs naturally with collection utilities such as `map` and `filter`:

```sema
(-> '(1 2 3 4)
    (filter odd?)
    (map (fn (x) (* x x))))
;; => (1 9)
```

**Note:** `->` is a prelude macro, so it is available automatically without an import.
