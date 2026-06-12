---
name: "dissoc"
module: "maps"
section: "Maps"
---

Remove a key, returning a new map. Works on both maps and hashmaps.

```sema
(dissoc {:a 1 :b 2} :a)                     ; => {:b 2}
(dissoc (hashmap/new :a 1 :b 2) :a)         ; hashmap without :a
```
