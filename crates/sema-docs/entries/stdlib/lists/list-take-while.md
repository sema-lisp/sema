---
name: "list/take-while"
module: "lists"
section: "Splitting"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["take-while", "list/drop-while", "take"]
---

Return the longest prefix of `seq` whose elements all satisfy `pred`. The walk
stops at the first element for which `pred` returns `#f`; elements after that
point are not examined even if they would satisfy the predicate. This is
different from `filter`, which keeps every matching element anywhere in the
sequence.

`take-while` is the same function under its Scheme name. The complement is
`list/drop-while`, and together they split a sequence at the first
non-matching element without scanning it twice.

```sema
(list/take-while even? '(2 4 5 6))                 ; => (2 4)
(list/take-while (fn (x) (< x 4)) '(1 2 3 4 5))    ; => (1 2 3)
(list/take-while even? '())                        ; => ()
(filter even? '(2 4 5 6))                          ; => (2 4 6)
```
