---
name: "merge"
module: "maps"
section: "Maps"
syntax: "(merge map ...)"
returns: "map"
see_also: ["deep-merge", "assoc", "map/zip"]
---

Merge multiple maps together, returning a new map. For keys present in more than one map, the **last** map wins. Works on both maps and hashmaps — the result type matches the first argument.

`merge` is shallow: when two maps share a key whose value is itself a map, the later value *replaces* the earlier one wholesale. To recurse into shared sub-maps instead, use `deep-merge`.

```sema
(merge {:a 1} {:b 2} {:c 3})       ; => {:a 1 :b 2 :c 3}
(merge {:a 1} {:a 99})             ; => {:a 99}  (last wins)
(merge (hashmap/new :a 1) {:b 2})  ; hashmap with :a and :b

;; Shallow — the nested map is replaced, not merged:
(merge {:a {:x 1}} {:a {:y 2}})    ; => {:a {:y 2}}
;; deep-merge would give {:a {:x 1 :y 2}}
```
