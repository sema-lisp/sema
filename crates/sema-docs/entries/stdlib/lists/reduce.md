---
name: "reduce"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function }, { name: seq, type: list }]
returns: "any"
see_also: ["foldl", "foldr", "fold", "apply"]
---

Combine a list into a single value left-to-right. Unlike `foldl`, you give no explicit seed — `reduce` uses the **first element** as the initial accumulator and folds the rest into it.

Because of that, `reduce` on an **empty list errors** (there is no first element to seed with); a single-element list returns that element untouched without ever calling the function. Reach for `foldl` when you need an explicit seed (e.g. summing into `0`, building a map) or must tolerate empty input.

```sema
(reduce + '(1 2 3 4 5))                  ; => 15
(reduce max '(3 1 4 1 5))                ; => 5
(reduce (fn (a b) (str a "-" b)) '("a" "b" "c"))  ; => "a-b-c"
(reduce + '(7))                          ; => 7    ; single element, fn never runs
(reduce + '())                           ; error: reduce: empty list
```
