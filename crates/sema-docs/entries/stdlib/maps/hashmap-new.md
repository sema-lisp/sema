---
name: "hashmap/new"
module: "maps"
section: "HashMaps"
syntax: "(hashmap/new key val ...)"
returns: "map"
---

Create a new hashmap from key-value pairs.

```sema
(hashmap/new :a 1 :b 2 :c 3)   ; create a hashmap
(hashmap/new)                    ; empty hashmap
```
