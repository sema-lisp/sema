---
name: "sort"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: lst, type: list }, { name: cmp, type: function, doc: "optional comparator" }]
returns: "list"
---

Return a new list sorted in ascending order. The input is left unchanged. Pass an optional two-argument comparator to control the order (e.g. `>` for descending).

Without a comparator, `sort` orders one type at a time. Numbers (ints and floats) sort together by numeric value; strings, symbols, keywords, chars, and booleans each sort within their own kind. Mixing unrelated types (e.g. a number and a string) is a type error rather than a silent, arbitrary order — pass an explicit comparator or use `sort-by` to order mixed types deliberately.

```sema
(sort '(3 1 4 1 5))                ; => (1 1 3 4 5)
(sort '(3 1.5 2))                  ; => (1.5 2 3)     ; ints and floats compare numerically
(sort '(3 1 4 1 5) >)              ; => (5 4 3 1 1)   ; custom comparator
(sort '("banana" "apple" "cherry")) ; => ("apple" "banana" "cherry")
(sort (list 1 "a"))               ; error: sort orders one type at a time
```

See also: `sort-by` (sort by a derived key rather than a comparator).
