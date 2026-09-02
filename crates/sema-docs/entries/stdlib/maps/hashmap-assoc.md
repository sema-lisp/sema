---
name: "hashmap/assoc"
module: "maps"
section: "HashMaps"
syntax: "(hashmap/assoc hm key value ...)"
returns: "hashmap"
see_also: ["hashmap/new", "hashmap/get", "assoc", "hashmap/to-map"]
---

Add or update a key-value pair in a hashmap, returning a new hashmap. The generic `assoc` works on hashmaps too; use this when you want to be explicit about the type.

```sema
(hashmap/assoc (hashmap/new) :a 1)   ; hashmap with :a 1
```
