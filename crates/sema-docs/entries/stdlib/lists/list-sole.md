---
name: "list/sole"
module: "lists"
section: "Filtering"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "any"
see_also: ["list/find", "filter", "list/unique"]
---

Return the one element matching the predicate, asserting that there is exactly one. It **errors** if zero match or if more than one matches — that strictness is the point: it turns a "should be unique" assumption into a checked invariant instead of silently taking the first hit.

Reach for `list/find` instead when zero-or-more matches are acceptable and you just want the first (it returns `nil` rather than erroring).

```sema
(list/sole (fn (x) (> x 4)) '(1 2 3 4 5))   ; => 5
(list/sole even? '(1 2 3 4 5))              ; error: more than one matching item
(list/sole even? '(1 3 5))                  ; error: no matching item
```
