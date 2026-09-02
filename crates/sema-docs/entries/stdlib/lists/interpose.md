---
name: "interpose"
module: "lists"
section: "Grouping"
params: [{ name: sep, type: any }, { name: list, type: list }]
returns: "list"
see_also: ["list/join", "list/interleave", "string/join"]
---

Return a new list with `sep` inserted *between* each pair of elements (not at the ends). A common precursor to joining: `interpose` then `apply string-append`.

```sema
(interpose ", " '("a" "b" "c"))   ; => ("a" ", " "b" ", " "c")
(interpose 0 '(1 2 3))            ; => (1 0 2 0 3)

;; Build a comma-separated string:
(apply string-append (interpose ", " '("a" "b" "c")))  ; => "a, b, c"
```

For the string-only case, `string/join` does the interpose-and-concatenate in one step.

