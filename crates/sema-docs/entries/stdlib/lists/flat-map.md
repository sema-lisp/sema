---
name: "flat-map"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["map", "flatten", "filter"]
---

Map `f` over a list, then splice the results together — `f` returns a list per element and `flat-map` concatenates them (flattening one level). Use it when each input expands to zero, one, or many outputs.

Because `f` can return an empty list, `flat-map` doubles as a combined filter-and-map: return `(list x)` to keep `x`, `'()` to drop it.

```sema
(flat-map (fn (x) (list x (* x 10))) '(1 2 3))
; => (1 10 2 20 3 30)

;; Expand each word into its characters:
(flat-map string->list '("ab" "cd"))
; => (#\a #\b #\c #\d)

;; Keep evens, drop odds (each element yields a 1- or 0-element list):
(flat-map (fn (x) (if (even? x) (list x) '())) '(1 2 3 4))
; => (2 4)
```

See also `map` (one output per input, no flattening) and `flatten` (flattens an existing nested list without mapping).

