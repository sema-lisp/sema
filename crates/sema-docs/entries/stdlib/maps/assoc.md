---
name: "assoc"
module: "maps"
section: "Maps"
syntax: "(assoc map key val ...)"
returns: "map"
---

Add or update key-value pairs, returning a **new** map; the original is never mutated.

Accepts any number of trailing key/value pairs, applied left to right. `assoc` only touches the top level — to reach into a nested map use `assoc-in`/`map/assoc-in`. To remove a key instead, use `dissoc`.

```sema
(assoc {:a 1} :b 2)          ; => {:a 1 :b 2}
(assoc {:a 1} :a 99)         ; => {:a 99}  (existing key replaced)
(assoc {} :a 1 :b 2 :c 3)    ; => {:a 1 :b 2 :c 3}  (multiple pairs)

;; The input is unchanged — assoc returns a fresh map.
(let ((m {:a 1}))
  (assoc m :b 2)
  m)                         ; => {:a 1}
```

## Association-list form

Called as `(assoc key alist)` — two arguments where the second is a list — `assoc`
instead does a classic Scheme association-list lookup: it scans a list of
`(key value)` pairs using `equal?` (structural) comparison and returns the **whole
matching pair** (reach for the value with `cadr`), or `#f` when no key matches.

```sema
(define alist '(("a" 1) ("b" 2) ("c" 3)))
(assoc "b" alist)         ; => ("b" 2)
(assoc "z" alist)         ; => #f
(cadr (assoc "b" alist))  ; => 2   (get the value, not the pair)
```

Because `#f` doubles as "not found", check membership before destructuring an
unknown key. For larger or mutable lookups, a hash map (`{...}` / `get`) is faster
than scanning an alist. See also `assq`/`assv` (other comparison flavors — in Sema
all three compare by value).
