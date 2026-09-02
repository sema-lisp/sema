---
name: "zip"
module: "lists"
section: "Sublists"
syntax: "(zip seq1 seq2 ...)"
returns: "list"
see_also: ["list/interleave", "map", "enumerate", "list/cross-join", "map/zip"]
---

Combine the sequences element-wise: the result's first element is a list of
every input's first element, and so on. With two inputs this makes pairs;
with three or more it makes triples and longer tuples. The result is as long
as the **shortest** input; extra elements of longer inputs are dropped.

`zip` is the usual way to walk two lists in lockstep with `map` or `for-each`,
or to build an association list from a key list and a value list. Its inverse
for pairs is `(apply zip pairs)`.

```sema
(zip '(1 2 3) '("a" "b" "c"))     ; => ((1 "a") (2 "b") (3 "c"))
(zip '(1 2 3) '(a b))             ; => ((1 a) (2 b))
(zip '(1 2) '(a b) '(x y))        ; => ((1 a x) (2 b y))
```

```sema
;; Pair up and combine.
(map (fn (p) (apply + p)) (zip '(1 2) '(10 20)))   ; => (11 22)

;; Unzip: transpose the pairs back into two lists.
(apply zip '((1 a) (2 b)))   ; => ((1 2) (a b))
```
