---
name: "hashmap/contains?"
module: "maps"
section: "HashMaps"
params: [{ name: hm, type: hashmap }, { name: key, type: any }]
returns: "bool"
see_also: ["hashmap/get", "contains?", "hashmap/keys"]
---

Test whether a hashmap contains a key. Like the generic `contains?`, this is the reliable way to detect a key whose stored value is `nil` (which `hashmap/get` can't distinguish from "absent").

```sema
(hashmap/contains? (hashmap/new :a 1) :a)   ; => #t
(hashmap/contains? (hashmap/new :a 1) :z)   ; => #f
```
