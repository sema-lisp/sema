---
name: "hashmap/get"
module: "maps"
section: "HashMaps"
params: [{ name: hm, type: hashmap }, { name: key, type: any }, { name: default, type: any, doc: "optional; defaults to nil" }]
returns: "any"
see_also: ["hashmap/assoc", "hashmap/contains?", "get"]
---

Look up a value in a hashmap, returning `nil` (or a third *default* argument) when the key is absent. The generic `get` also works on hashmaps, so you rarely need this explicitly.

```sema
(hashmap/get (hashmap/new :a 1) :a)        ; => 1
(hashmap/get (hashmap/new :a 1) :z)        ; => nil
(hashmap/get (hashmap/new :a 1) :z "d")    ; => "d"
```
