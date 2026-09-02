---
name: "frequencies"
module: "lists"
section: "Grouping"
params: [{ name: list, type: list }]
returns: "map"
see_also: ["list/group-by", "list/mode", "list/duplicates", "count"]
---

Count occurrences of each distinct element, returning a map from element to its count. The one-step way to build a histogram or tally.

```sema
(frequencies '(a b a c b a))             ; => {a 3 b 2 c 1}
(get (frequencies '(a b a c b a)) 'a)    ; => 3

;; Pair with string->list for a character tally:
(frequencies (string->list "hello"))     ; => {#\e 1 #\h 1 #\l 2 #\o 1}
```

See also `list/group-by` (buckets the original elements by a key function, rather than just counting them).

