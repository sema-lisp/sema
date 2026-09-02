---
name: "hashmap/new"
module: "maps"
section: "HashMaps"
syntax: "(hashmap/new key val ...)"
returns: "map"
see_also: ["hashmap/assoc", "hashmap/to-map", "hash-map", "hash-map?"]
---

Create a hashmap from alternating key/value arguments. A hashmap is an **unordered** hash table — a distinct type from the ordered `{...}` map literal (which keeps keys sorted for display). Reach for a hashmap when you don't care about key order and want fast lookups; use `hashmap/to-map` or `map/sort-keys` to get a sorted, printable map back.

```sema
(hashmap/new :a 1 :b 2 :c 3)   ; create a hashmap
(hashmap/new)                  ; empty hashmap
```
