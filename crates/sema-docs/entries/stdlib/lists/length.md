---
name: "length"
module: "lists"
section: "Basic Operations"
params: [{ name: list, type: list }]
returns: "int"
---

Return the number of elements in a collection. Works on lists, vectors, strings, maps, bytevectors, and typed arrays. See also `count`, which additionally treats `nil` as `0`.

```sema
(length '(1 2 3))  ; => 3
(length '())       ; => 0
(length [10 20 30]); => 3
(length "abc")     ; => 3
```
