---
name: "any"
module: "lists"
section: "Searching"
params: [{ name: pred, type: function }, { name: lst, type: list }]
returns: "bool"
see_also: ["any?", "every", "some?", "list/find"]
---

Return `#t` if `pred` is truthy for at least one element; `#f` otherwise (including for the empty list). Stops at the first match. Alias: `any?`.

```sema
(any even? '(1 3 5 6))   ; => #t
(any even? '(1 3 5))     ; => #f
(any even? '())          ; => #f
```

The mirror of `every` (all elements must pass). Note the empty-list base cases differ: `any` is `#f` (no witness), `every` is `#t` (vacuously true).

