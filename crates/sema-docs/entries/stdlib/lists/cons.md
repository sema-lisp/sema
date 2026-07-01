---
name: "cons"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: any }, { name: lst, type: list }]
returns: "list"
---

Prepend an element to a list.

Sema has **no dotted pairs / improper lists** — `cons` always yields a proper
list. If the second argument isn't a list it's treated as a one-element list, so
`(cons a b)` becomes the two-element list `(a b)`, and `cdr` returns a list, not a
bare value. To build a pair, use `(list a b)` and read it back with `car`/`nth`
(not `car`/`cdr`), e.g. `(nth p 0)` / `(nth p 1)`.

```sema
(cons 0 '(1 2 3))  ; => (0 1 2 3)
(cons 1 '())       ; => (1)
(cons 0 "a")       ; => (0 "a")      ; not a dotted pair
(cdr (cons 0 "a")) ; => ("a")        ; a list, not "a"
```
