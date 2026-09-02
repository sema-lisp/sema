---
name: "last"
module: "lists"
section: "Construction & Access"
params: [{ name: seq, type: "list | vector" }]
returns: "any or nil"
see_also: ["first", "list/take-last", "nth", "car"]
---

Return the final element of a list or vector. An empty sequence gives `nil`
rather than an error, so a call on possibly-empty input needs no guard; use
`(empty? xs)` first when `nil` is also a legitimate element.

Lists are linked, so `last` walks the whole list (O(n)); `first` is O(1).
For the last *n* elements use `list/take-last`.

```sema
(last '(1 2 3))            ; => 3
(last [1 2])               ; => 2
(last '())                 ; => nil
(list/take-last 2 '(1 2 3 4))   ; => (3 4)
```
