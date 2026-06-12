---
name: "assoc"
module: "maps"
section: "Maps"
---

Add or update a key-value pair, returning a new map.

```sema
(assoc {:a 1} :b 2)     ; => {:a 1 :b 2}
(assoc {:a 1} :a 99)    ; => {:a 99}
```
