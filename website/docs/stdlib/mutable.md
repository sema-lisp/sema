---
outline: [2, 3]
---

# Mutable Containers

Sema's default collections — lists, vectors, maps — are **immutable**: every "update" produces a new value (copy-on-write), and that is the right default for almost all code. Mutable containers are the imperative escape hatch for the cases where copy-on-write dominates the runtime: hot accumulation loops, in-place statistics, a counter threaded through callbacks.

There are two of them:

- **`mutable-array`** — an in-place mutable array of values (Janet-style)
- **`mutable-cell`** — a single in-place mutable slot holding one value (a boxed value)

Both are **shared by reference**: mutating through one binding is visible through every other binding to the same container. When the loop is done, freeze the result with `mutable-array/->vector` to hand data back to the immutable world.

## When to Reach for Them

Prefer the immutable structures plus `foldl`/`reduce` by default — they are safe to share, safe to use as map keys, and fast enough for typical workloads. Reach for a mutable container when:

- **A hot loop accumulates into a growing collection.** Pushing onto a `mutable-array` is a true in-place append; rebuilding a persistent vector per element copies.
- **You update a fixed set of slots millions of times.** In-place stats like `[min max sum count]` per key: `mutable-array/set!` overwrites one slot with no copy.
- **A running value must survive across callbacks.** A `mutable-cell` gives a counter or "best so far" that side-effecting callbacks (`for-each`, event handlers) can update without rebuilding a container per event.

## Semantics

**Reference sharing** — mutable containers are heap values shared by reference. Passing one to a function passes the *same* container; mutations are visible to every holder.

**Equality and ordering are content-based.** `equal?` compares elements (with an identity fast path — a container is trivially equal to itself), and ordering functions like `sort` order by contents, just like vectors. The comparison is cycle-safe: an array that contains itself compares without hanging.

```sema
(define a (mutable-array/new))
(define b (mutable-array/new))
(mutable-array/push! a 1)
(mutable-array/push! b 1)
(equal? a b)   ; => #t — same contents
```

**Mutable containers cannot be map keys.** Map keys must be deeply immutable, and the check is deep — a vector *containing* a mutable array is rejected too, because the key could still mutate underneath the map and corrupt its ordering.

```sema
(assoc {} (mutable-array/new) 1)
; => error: expected immutable map key, got mutable-array
;    hint: freeze the key first (mutable-array/->vector or mutable-cell/get)
```

Mutable containers as map *values* are fine — that is the standard pattern for per-key accumulators (see the example below).

**Printing shows length only** — a mutable array prints as `<mutable-array 3>` (it can contain itself, so contents are not printed) and a cell as `<mutable-cell>`. Freeze with `mutable-array/->vector` when you want to see or return the elements.

**`nth` works on mutable arrays**, like it does on lists and vectors.

## Mutable Arrays

### `mutable-array/new`

Create a mutable array. With no arguments it is empty; with one argument it is still empty but pre-allocates capacity for that many pushes; with two arguments it holds `n` copies of `fill`, ready for indexed `mutable-array/set!`.

```sema
(mutable-array/new)        ; empty
(mutable-array/new 1024)   ; empty, capacity for 1024 pushes
(mutable-array/new 3 0)    ; three zeros: contents [0 0 0]
```

### `mutable-array/push!`

Append a value to the end, in place. Returns the array itself, so pushes chain and work as the accumulator of a fold.

```sema
(define a (mutable-array/new))
(mutable-array/push! (mutable-array/push! a 1) 2)
(mutable-array/->vector a)   ; => [1 2]

;; As a fold accumulator:
(mutable-array/->vector
  (foldl (fn (acc x) (mutable-array/push! acc (* x x)))
         (mutable-array/new)
         '(1 2 3)))
; => [1 4 9]
```

### `mutable-array/get`

Read the element at a zero-based index. Out of bounds is an error unless a default is supplied.

```sema
(define a (mutable-array/new 2 :x))
(mutable-array/get a 1)            ; => :x
(mutable-array/get a 9 :missing)   ; => :missing
```

### `mutable-array/set!`

Overwrite the element at a zero-based index, in place. The slot must already exist (`index < length`) — use `mutable-array/push!` to grow. Returns the array. Unlike `vector` updates, no copy is made: every binding to the array sees the new value.

```sema
(define stats (mutable-array/new 4 0))   ; [min max sum count] accumulator
(mutable-array/set! stats 2 (+ (mutable-array/get stats 2) 57))
(mutable-array/->vector stats)   ; => [0 0 57 0]
```

### `mutable-array/length`

Return the number of elements currently in the array (not its capacity).

```sema
(mutable-array/length (mutable-array/new 64))    ; => 0 (capacity only)
(mutable-array/length (mutable-array/new 3 :x))  ; => 3
```

### `mutable-array/->vector`

Freeze a mutable array into an immutable vector — a snapshot copy: later mutation of the array does not change the returned vector. This is the hand-off point from an imperative accumulation loop back to the persistent world (sortable, printable, usable as map values or keys).

```sema
(define a (mutable-array/new))
(mutable-array/push! a 1)
(define v (mutable-array/->vector a))
(mutable-array/set! a 0 9)
v   ; => [1] — the snapshot is unaffected
```

## Mutable Cells

### `mutable-cell/new`

Create a mutable cell holding one value.

```sema
(define counter (mutable-cell/new 0))
```

### `mutable-cell/get`

Read the current contents of a cell.

```sema
(mutable-cell/get (mutable-cell/new :ready))   ; => :ready
```

### `mutable-cell/set!`

Replace the contents of a cell, in place. Returns the cell. Every binding to the cell sees the new value.

```sema
(define counter (mutable-cell/new 0))
(mutable-cell/set! counter (+ 1 (mutable-cell/get counter)))
(mutable-cell/get counter)   ; => 1
```

## Example: Per-Key Stats in a Fold

The canonical use case: fold over a large file of `Name;-12.3` measurement lines, keeping `[min max sum count]` per station. The map is immutable, but each station's stats live in one mutable array that is updated in place — the map itself only changes when a *new* station appears. Combined with [`file/fold-lines-bytes`](/docs/stdlib/file-io#file-fold-lines-bytes) and [`bytes/*`](/docs/stdlib/bytevectors#byte-oriented-operations) parsing, the per-line work allocates almost nothing.

```sema
;; [min max sum count], all ints ×10 — one allocation per station.
(define (stats-new x)
  (mutable-array/set! (mutable-array/new 4 x) 3 1))   ; set! returns the array

(define (stats-add! s x)
  (mutable-array/set! s 0 (min (mutable-array/get s 0) x))
  (mutable-array/set! s 1 (max (mutable-array/get s 1) x))
  (mutable-array/set! s 2 (+ (mutable-array/get s 2) x))
  (mutable-array/set! s 3 (+ (mutable-array/get s 3) 1)))

(define stats
  (file/fold-lines-bytes "measurements.txt"
    (fn (acc line)
      (let* ((semi (bytes/find line 59))                 ; 59 = ';'
             (name (bytes/->string line 0 semi))
             (temp (bytes/parse-int10 line (+ semi 1)))
             (s    (get acc name)))
        (if (nil? s)
            (assoc acc name (stats-new temp))    ; new station: map grows once
            (begin (stats-add! s temp) acc))))   ; known station: mutate in place
    {}))

;; Freeze for the immutable world before printing or returning.
(map/map-vals mutable-array/->vector stats)
; => {"Bergen" [59 59 59 1] "Oslo" [-123 30 -93 2]}
```

## Example: Counting Across Callbacks

A `mutable-cell` threads a running value through side-effecting iteration without rebuilding anything per step:

```sema
(define matches (mutable-cell/new 0))

(file/for-each-line "app.log"
  (fn (line)
    (when (string/contains? line "ERROR")
      (mutable-cell/set! matches (+ 1 (mutable-cell/get matches))))))

(mutable-cell/get matches)   ; => number of ERROR lines
```
