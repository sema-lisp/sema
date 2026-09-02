---
name: "list/shuffle"
module: "lists"
section: "Random"
params: [{ name: seq, type: list }]
returns: "list"
see_also: ["list/pick", "sort", "list/unique"]
---

Return a new list with the elements of `seq` in uniformly random order (a
Fisher-Yates shuffle from the process's random generator). The input is not
modified, every element appears exactly once, and an empty sequence gives
`()`.

The order is not seedable from Sema code, so a test should assert on
properties of the result (length, membership, sorted form) rather than on a
particular order. To pick a single random element use `list/pick`; to pick
`n` distinct elements, take from a shuffle.

```sema
(list/shuffle '(1 2 3 4 5))              ; => varies, e.g. (3 1 5 2 4)
(sort (list/shuffle '(3 1 2)))           ; => (1 2 3)
(length (list/shuffle '(1 2 3 4)))       ; => 4
(list/shuffle '())                       ; => ()
(take 2 (list/shuffle (quote (a b c d))))   ; => varies: two distinct elements
```
