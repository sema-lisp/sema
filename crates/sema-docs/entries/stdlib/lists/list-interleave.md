---
name: "list/interleave"
module: "lists"
section: "Grouping"
syntax: "(list/interleave seq1 seq2 ...)"
returns: "list"
see_also: ["zip", "interpose", "list/cross-join"]
---

Merge sequences by taking one element from each in turn: the first element of
every input, then the second of every input, and so on. Output stops when the
**shortest** input runs out, so the result length is the shortest length
times the number of inputs.

The difference from `zip` is the shape: `zip` groups the elements into
sub-lists, `list/interleave` flattens them into one list. Use `interpose` to
put a single separator value between elements instead.

```sema
(list/interleave '(1 2 3) '(a b c))      ; => (1 a 2 b 3 c)
(list/interleave '(1 2 3) '(a))          ; => (1 a)
(list/interleave '(1 2) '(a b) '(x y))   ; => (1 a x 2 b y)
(zip '(1 2) '(a b))                      ; => ((1 a) (2 b))
```
