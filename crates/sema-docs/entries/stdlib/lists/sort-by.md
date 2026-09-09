---
name: "sort-by"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: key-fn, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["sort", "reverse", "list/group-by"]
---

Return a new list sorted by the value the key function produces for each element, in ascending order. The key is computed once per element and the *elements* are returned (not the keys), so this is the idiomatic way to sort maps/records by a field.

Real numeric keys compare by numeric value across integer, float, and rational
types. NaN sorts after other real numbers. Equal keys retain their input order.
For mixed key types, real numbers form one group at the integer type's position;
other values use Sema's total value order, including complex numbers.

```sema
(sort-by length '("bb" "a" "ccc"))   ; => ("a" "bb" "ccc")
(sort-by abs '(-3 1 -2))             ; => (1 -2 -3)

(sort-by (fn (p) (get p :age))
         (list {:name "Alice" :age 30} {:name "Bob" :age 25}))
; => ({:age 25 :name "Bob"} {:age 30 :name "Alice"})
```

See also: `sort` (plain ascending or with a custom comparator).
