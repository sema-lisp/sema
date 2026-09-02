---
name: "dissoc"
module: "maps"
section: "Maps"
syntax: "(dissoc m key ...)"
returns: "map"
see_also: ["assoc", "map/except", "map/select-keys"]
---

Remove one or more keys, returning a new map; the original is untouched. Works on both maps and hashmaps. Removing a key that isn't present is a no-op (no error). Inverse of `assoc`.

```sema
(dissoc {:a 1 :b 2} :a)              ; => {:b 2}
(dissoc {:a 1 :b 2 :c 3} :a :c)      ; => {:b 2}  (multiple keys)
(dissoc {:a 1} :missing)             ; => {:a 1}  (absent key ignored)
(dissoc (hashmap/new :a 1 :b 2) :a)  ; hashmap without :a
```
