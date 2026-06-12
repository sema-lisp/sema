---
name: "for/map"
module: "special-forms"
syntax: "(for/map ((var expr) ...) body)"
---

Map comprehension: evaluate the body for each combination of elements across the given sequences and collect the results into a hash map. The body must produce a key-value pair, typically using `values` or `list`. Each binding pairs a variable with a sequence expression; multiple bindings produce nested iteration (cartesian product).

Like `for/list`, `for/map` supports `:when` filter clauses to skip iterations conditionally. If multiple iterations produce the same key, later values overwrite earlier ones because the result is a hash map. This form is ideal for building lookup tables, inverted indexes, or any derived mapping from one or more input sequences.

```sema
(for/map ((x '(1 2 3)))
  (values x (* x x)))
;; => {1 1, 2 4, 3 9}
```

```sema
(for/map ((c "abc"))
  (values (str c) (string/to-keyword (str c))))
;; => {"a" :a, "b" :b, "c" :c}
```

```sema
(for/map ((n (range 1 11))
          (:when (odd? n)))
  (values n (* n n)))
;; => {1 1, 3 9, 5 25, 7 49, 9 81}
```

**Note:** `for/map` is typically implemented as a macro that expands to `for/list` followed by `foldl` with `assoc`. It is not a core special form and may need to be defined or imported from a comprehension library.