---
name: "apply"
module: "lists"
section: "Higher-Order Functions"
syntax: "(apply f arg ... lst)"
returns: "any"
see_also: ["map", "fold", "values", "call-with-values"]
---

Call a function with the elements of a list spread out as its arguments. `(apply f xs)` is `(f x1 x2 ...)` — the bridge between "I have a list of values" and "this function takes separate arguments".

Any leading arguments before the final list are passed first, then the list is spliced on the end; the last argument must be a list.

```sema
(apply + '(1 2 3))     ; => 6
(apply max '(3 1 4))   ; => 4

;; Leading fixed args, then the list:
(apply + 1 2 '(3 4))   ; => 10

;; Common use: forward a collected arg list to a variadic builtin
(define nums '(5 9 2))
(apply min nums)       ; => 2
```

