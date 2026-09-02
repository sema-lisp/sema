---
outline: [2, 3]
---

# Maps & HashMaps

Sema provides two map types: sorted **maps** (BTreeMap-backed, deterministic ordering) and **hashmaps** (for O(1) performance-critical lookups).

## Maps

Maps use curly-brace literal syntax with keyword keys:

```sema
{:name "Ada" :age 36}   ; map literal
{:a 1 :b 2 :c 3}       ; keywords as keys
```

Keywords are callable — when used as a function, they look up their value in a map:

```sema
(:name {:name "Ada" :age 36})   ; => "Ada"
```

### `map/new`

Create a map from key-value pairs.

```sema
(map/new :a 1 :b 2)   ; => {:a 1 :b 2}
```

### `get`

Look up a value by key. Works on both maps and hashmaps. A third argument is the
default for a missing key. Keywords in operator position do the same lookup:
`(:a m)` and `(:a m default)`.

```sema
(get {:a 1 :b 2} :a)          ; => 1
(get {:a 1 :b 2} :z)          ; => nil
(get {:a 1 :b 2} :z 0)        ; => 0
(:b {:a 1 :b 2})              ; => 2
```

`get`, `assoc`, `dissoc`, and `merge` accept maps only: `(get nil :a)` and
`(get '(1 2) 0)` are type errors. Index a list or vector with `nth`.

### `assoc`

Add or update a key-value pair, returning a new map.

```sema
(assoc {:a 1} :b 2)     ; => {:a 1 :b 2}
(assoc {:a 1} :a 99)    ; => {:a 99}
```

### `dissoc`

Remove a key, returning a new map. Works on both maps and hashmaps.

```sema
(dissoc {:a 1 :b 2} :a)                     ; => {:b 2}
(dissoc (hashmap/new :a 1 :b 2) :a)         ; hashmap without :a
```

### `merge`

Merge multiple maps together. Later maps override earlier ones. Works on both maps and hashmaps — the result type matches the first argument.

```sema
(merge {:a 1} {:b 2} {:c 3})   ; => {:a 1 :b 2 :c 3}
(merge {:a 1} {:a 99})         ; => {:a 99}
(merge (hashmap/new :a 1) {:b 2})  ; hashmap with :a and :b
```

### `keys`

Return the keys of a map as a list.

```sema
(keys {:a 1 :b 2})   ; => (:a :b)
```

### `vals`

Return the values of a map as a list.

```sema
(vals {:a 1 :b 2})   ; => (1 2)
```

### `contains?`

Test if a map contains a key.

```sema
(contains? {:a 1} :a)   ; => #t
(contains? {:a 1} :b)   ; => #f
```

### `count`

Return the number of key-value pairs.

```sema
(count {:a 1 :b 2})   ; => 2
```

### `map/entries`

Return the entries as a list of key-value pairs.

```sema
(map/entries {:a 1 :b 2})   ; => ((:a 1) (:b 2))
```

### `map/from-entries`

Create a map from a list of key-value pairs.

```sema
(map/from-entries '((:a 1) (:b 2)))   ; => {:a 1 :b 2}
```

## Higher-Order Map Operations

### `map/map-vals`

Apply a function to every value in a map.

```sema
(map/map-vals (fn (v) (* v 2)) {:a 1 :b 2})   ; => {:a 2 :b 4}
```

### `map/map-keys`

Apply a function to every key in a map.

```sema
(map/map-keys
  (fn (k) (string/to-keyword (string/upper (keyword/to-string k))))
  {:a 1})
; => {:A 1}
```

### `map/filter`

Filter entries by a predicate that takes key and value.

```sema
(map/filter (fn (k v) (> v 1)) {:a 1 :b 2 :c 3})   ; => {:b 2 :c 3}
```

### `map/select-keys`

Select only the given keys from a map.

```sema
(map/select-keys {:a 1 :b 2 :c 3} '(:a :c))   ; => {:a 1 :c 3}
```

### `map/update`

Update a value at a key by applying a function.

```sema
(map/update {:a 1} :a (fn (v) (+ v 10)))   ; => {:a 11}
```

## HashMaps

For performance-critical workloads with many keys, use `hashmap` for O(1) lookups instead of the sorted `map`.

### `hashmap/new`

Create a new hashmap from key-value pairs.

```sema
(hashmap/new :a 1 :b 2 :c 3)   ; create a hashmap
(hashmap/new)                    ; empty hashmap
```

### `hashmap/get`

Look up a value in a hashmap.

```sema
(hashmap/get (hashmap/new :a 1) :a)   ; => 1
```

### `hashmap/assoc`

Add a key-value pair to a hashmap.

```sema
(hashmap/assoc (hashmap/new) :a 1)   ; hashmap with :a 1
```

### `hashmap/to-map`

Convert a hashmap to a sorted map.

```sema
(hashmap/to-map (hashmap/new :b 2 :a 1))   ; => {:a 1 :b 2}
```

### `hashmap/keys`

Return the keys of a hashmap (unordered).

```sema
(hashmap/keys (hashmap/new :a 1 :b 2))   ; => (:a :b)
```

### `hashmap/contains?`

Test if a hashmap contains a key.

```sema
(hashmap/contains? (hashmap/new :a 1) :a)   ; => #t
```

### Generic Operations on HashMaps

The generic functions `get`, `assoc`, `dissoc`, `keys`, `vals`, `merge`, `count`, `contains?`, and all `map/*` higher-order operations also work on hashmaps, preserving the hashmap type:

```sema
(get (hashmap/new :a 1 :b 2) :a)       ; => 1
(assoc (hashmap/new) :x 42)            ; hashmap with :x 42
(dissoc (hashmap/new :a 1 :b 2) :a)    ; hashmap without :a
(merge (hashmap/new :a 1) {:b 2})      ; hashmap with :a and :b
(count (hashmap/new :a 1 :b 2))        ; => 2
(map/map-vals (fn (v) (* v 2)) (hashmap/new :a 1))  ; hashmap with :a 2
(map/filter (fn (k v) (> v 1)) (hashmap/new :a 1 :b 2))  ; hashmap with :b
```

### `map/sort-keys`

Sort a map by its keys. Converts hashmaps to sorted maps.

```sema
(map/sort-keys (hashmap/new :c 3 :a 1 :b 2))   ; => {:a 1 :b 2 :c 3}
```

### `map/except`

Remove specified keys from a map (inverse of `map/select-keys`).

```sema
(map/except {:a 1 :b 2 :c 3} '(:b))       ; => {:a 1 :c 3}
(map/except {:a 1 :b 2 :c 3} '(:a :c))    ; => {:b 2}
```

### `map/zip`

Create a map from a list of keys and a list of values.

```sema
(map/zip '(:a :b :c) '(1 2 3))   ; => {:a 1 :b 2 :c 3}
```

## Nested Map Operations

### `map/get-in`

Access a value at a nested key path. Returns `nil` (or a default) if any key is missing.

```sema
(map/get-in {:a {:b {:c 42}}} [:a :b :c])           ; => 42
(map/get-in {:a {:b 1}} [:a :c])                     ; => nil
(map/get-in {:a {:b 1}} [:a :c] "default")           ; => "default"
```

### `map/assoc-in`

Set a value at a nested key path. Creates intermediate maps if they don't exist.

```sema
(map/assoc-in {:a {:b 1}} [:a :b] 42)                ; => {:a {:b 42}}
(map/assoc-in {} [:a :b :c] 99)                      ; => {:a {:b {:c 99}}}
```

### `map/update-in`

Update a value at a nested key path by applying a function.

```sema
(map/update-in {:a {:b 10}} [:a :b] #(+ % 1))       ; => {:a {:b 11}}
```

### `map/deep-merge`

Recursively merge maps. Nested maps are merged rather than replaced. Non-map values in the overlay override the base.

```sema
(map/deep-merge {:a {:b 1 :c 2}} {:a {:b 99}})      ; => {:a {:b 99 :c 2}}
(map/deep-merge {:a {:b 1}} {:a 42})                 ; => {:a 42}
(map/deep-merge {:a 1} {:b 2} {:c 3})               ; => {:a 1 :b 2 :c 3}
```
