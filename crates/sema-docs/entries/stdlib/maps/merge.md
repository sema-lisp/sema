---
name: "merge"
module: "maps"
section: "Maps"
syntax: "(merge map ...)"
returns: "map"
---

Merge multiple maps together. Later maps override earlier ones. Works on both maps and hashmaps — the result type matches the first argument.

```sema
(merge {:a 1} {:b 2} {:c 3})   ; => {:a 1 :b 2 :c 3}
(merge {:a 1} {:a 99})         ; => {:a 99}
(merge (hashmap/new :a 1) {:b 2})  ; hashmap with :a and :b
```
