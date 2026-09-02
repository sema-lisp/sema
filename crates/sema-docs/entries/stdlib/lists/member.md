---
name: "member"
module: "lists"
section: "Searching"
params: [{ name: value, type: any }, { name: seq, type: list }]
returns: "list or #f"
see_also: ["list/contains?", "list/index-of", "assq", "list/find"]
---

Return the **tail** of the list starting at the first element equal to the value, or `#f` if absent. This is the Scheme convention: a non-`#f` result is the sublist, not the position.

The returned tail is still truthy, so `member` doubles as a membership test inside `if`/`when` — just don't treat the result as the index. When you actually want the position, use `list/index-of`.

```sema
(member 3 '(1 2 3 4))                    ; => (3 4)
(member 9 '(1 2 3))                      ; => #f
(if (member 2 '(1 2 3)) "yes" "no")      ; => "yes"   ; tail is truthy
(list/index-of '(1 2 3 4) 3)             ; => 2        ; want the index instead
```
