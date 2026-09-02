---
name: "list/drop-while"
module: "lists"
section: "Splitting"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["drop-while", "list/take-while", "drop"]
---

Return `seq` without its longest prefix of elements that satisfy `pred`. The
first element for which `pred` returns `#f` starts the result, and everything
after it is kept unchanged, including later elements that would satisfy the
predicate. Compare `list/reject`, which removes matching elements anywhere.

`drop-while` is the same function under its Scheme name. Pair it with
`list/take-while` to split at the first non-matching element.

```sema
(list/drop-while even? '(2 4 5 6))                 ; => (5 6)
(list/drop-while (fn (x) (< x 4)) '(1 2 3 4 5))    ; => (4 5)
(list/drop-while even? '(1 2))                     ; => (1 2)
(list/reject even? '(2 4 5 6))                     ; => (5)
```

```sema
;; Skip leading blank lines of a document.
(list/drop-while string/empty? '("" "" "title" "" "body"))
; => ("title" "" "body")
```
