---
name: "list/sum"
module: "lists"
section: "Aggregation"
params: [{ name: seq, type: "list | vector" }]
returns: "number"
see_also: ["list/avg", "list/max", "list/min", "foldl"]
---

Add every number in the sequence. An empty sequence sums to `0`. The addition
runs through the same numeric tower as `+`, so the result type follows the
inputs: integers stay exact and promote to bignums past 64 bits, a rational
operand gives an exact rational, and any float makes the result a float. A
non-number element is a type error.

`(list/sum xs)` is `(apply + xs)` with a name that reads better in a
pipeline. For an average see `list/avg`; to sum a derived quantity, `map`
first.

```sema
(list/sum '(1 2 3 4 5))       ; => 15
(list/sum '())                ; => 0
(list/sum '(1 2.5))           ; => 3.5
(list/sum (list 1/2 1/2))     ; => 1
(list/sum [1 2])              ; => 3
(list/sum (map :price (list {:price 2} {:price 3})))   ; => 5
```
