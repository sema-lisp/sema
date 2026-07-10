---
outline: [2, 3]
---

# Lists

Lists are the fundamental data structure in Sema. They are built from cons pairs and support a rich set of operations.

## Construction & Access

### `list`

Create a new list.

```sema
(list 1 2 3)       ; => (1 2 3)
(list)             ; => ()
(list "a" "b")     ; => ("a" "b")
```

### `cons`

Prepend an element to a list.

```sema
(cons 0 '(1 2 3))  ; => (0 1 2 3)
(cons 1 '())       ; => (1)
```

### `car`

Return the first element of a list.

```sema
(car '(1 2 3))     ; => 1
```

### `cdr`

Return the rest of a list (everything after the first element).

```sema
(cdr '(1 2 3))     ; => (2 3)
(cdr '(1))         ; => ()
```

::: details Where these names come from
`car` and `cdr` are inherited from the [IBM 704](http://bitsavers.informatik.uni-stuttgart.de/pdf/ibm/704/24-6661-2_704_Manual_1955.pdf) (1955), the machine Lisp was originally implemented on. The 704 stored cons cells in a single 36-bit word, with two 15-bit pointer fields: the **address** field (bits 21-35) pointed to the first element, and the **decrement** field (bits 3-17) pointed to the rest of the list. `car` stands for "Contents of the Address Register" and `cdr` for "Contents of the Decrement Register" — they were single hardware instructions that extracted these sub-fields. Sema also provides `first`/`rest` as more readable aliases.
:::

### `first`

Alias for `car`. Return the first element.

```sema
(first '(1 2 3))   ; => 1
```

### `rest`

Alias for `cdr`. Return the rest of the list.

```sema
(rest '(1 2 3))    ; => (2 3)
```

### `cadr`, `caddr`, ...

Compositions of `car` and `cdr`. Available: `caar`, `cadr`, `cdar`, `cddr`, `caaar`, `caadr`, `cadar`, `caddr`, `cdaar`, `cdadr`, `cddar`, `cdddr`.

```sema
(cadr '(1 2 3))    ; => 2
(caddr '(1 2 3))   ; => 3
```

### `last`

Return the last element of a list.

```sema
(last '(1 2 3))    ; => 3
```

### `nth`

Return the element at index N (zero-based).

```sema
(nth '(10 20 30) 1)   ; => 20
(nth '(10 20 30) 0)   ; => 10
```

## Association Lists

### `assoc`

Look up a key in an association list (list of pairs). Uses `equal?` comparison.

```sema
(define alist '(("a" 1) ("b" 2) ("c" 3)))
(assoc "b" alist)   ; => ("b" 2)
(assoc "z" alist)   ; => #f
```

### `assq`

Like `assoc` but uses `eq?` comparison (pointer/symbol equality).

```sema
(assq 'b '((a 1) (b 2)))   ; => (b 2)
```

### `assv`

Find the first pair whose key equals `key`. In Sema this compares by value, so `assv`, `assq`, and `assoc` all match structurally equal keys (including compound keys) — they are not distinguished by object identity the way Scheme's `eqv?`/`eq?` would be.

```sema
(assv 2 '((1 "one") (2 "two")))   ; => (2 "two")
```

## Basic Operations

### `length`

Return the number of elements in a list.

```sema
(length '(1 2 3))  ; => 3
(length '())       ; => 0
```

### `append`

Concatenate lists.

```sema
(append '(1 2) '(3 4))     ; => (1 2 3 4)
(append '(1) '(2) '(3))    ; => (1 2 3)
```

### `reverse`

Reverse a list.

```sema
(reverse '(1 2 3))   ; => (3 2 1)
```

### `range`

Generate a list of integers. With one argument, generates 0 to N-1. With two, generates from start to end-1.

```sema
(range 5)       ; => (0 1 2 3 4)
(range 1 5)     ; => (1 2 3 4)
```

## Higher-Order Functions

### `map`

Apply a function to each element of one or more lists.

```sema
(map (fn (x) (* x x)) '(1 2 3))      ; => (1 4 9)
(map + '(1 2 3) '(10 20 30))          ; => (11 22 33)
```

### `map-indexed`

Like `map`, but calls the function with the index and the element: `(f index element)`.

```sema
(map-indexed (fn (i x) (list i x)) '(10 20 30))   ; => ((0 10) (1 20) (2 30))
```

### `enumerate`

Pair each element with its 0-based index.

```sema
(enumerate '(10 20 30))   ; => ((0 10) (1 20) (2 30))
```

### `filter`

Return elements that satisfy a predicate.

```sema
(filter even? '(1 2 3 4 5))   ; => (2 4)
(filter string? '(1 "a" 2))   ; => ("a")
```

### `foldl`

Left fold. `(foldl f init list)` — accumulates from left to right.

```sema
(foldl + 0 '(1 2 3 4 5))   ; => 15
(foldl cons '() '(1 2 3))  ; => (3 2 1)
```

### `foldr`

Right fold. `(foldr f init list)` — accumulates from right to left.

```sema
(foldr cons '() '(1 2 3))   ; => (1 2 3)
```

### `reduce`

Like `foldl` but uses the first element as the initial value.

```sema
(reduce + '(1 2 3 4 5))   ; => 15
```

### `for-each`

Apply a function to each element for side effects.

```sema
(for-each println '("a" "b" "c"))
;; prints: a, b, c (each on a new line)
```

### `sort`

Sort a list in ascending order.

```sema
(sort '(3 1 4 1 5))   ; => (1 1 3 4 5)
```

### `sort-by`

Sort a list by a key function.

```sema
(sort-by length '("bb" "a" "ccc"))   ; => ("a" "bb" "ccc")
(sort-by abs '(-3 1 -2))             ; => (1 -2 -3)
```

### `flat-map`

Map a function over a list and flatten the results by one level.

```sema
(flat-map (fn (x) (list x (* x 10))) '(1 2 3))
; => (1 10 2 20 3 30)
```

### `apply`

Apply a function to a list of arguments.

```sema
(apply + '(1 2 3))   ; => 6
(apply max '(3 1 4)) ; => 4
```

## Sublists

### `take`

Take the first N elements.

```sema
(take 3 '(1 2 3 4 5))   ; => (1 2 3)
(take 10 '(1 2))         ; => (1 2)
```

### `drop`

Drop the first N elements.

```sema
(drop 2 '(1 2 3 4 5))   ; => (3 4 5)
```

### `list/take-last`

Take the last N elements (the tail counterpart to `take`). Clamps to the list length.

```sema
(list/take-last 2 '(1 2 3 4))   ; => (3 4)
(list/take-last 9 '(1 2))       ; => (1 2)
```

### `list/drop-last`

Drop the last N elements (drops from the tail; the counterpart to `drop`). Clamps to empty.

```sema
(list/drop-last 2 '(1 2 3 4))   ; => (1 2)
(list/drop-last 9 '(1 2))       ; => ()
```

### `flatten`

Flatten nested lists into a single list.

```sema
(flatten '(1 (2 (3)) 4))   ; => (1 2 3 4)
```

### `flatten-deep`

Recursively flatten all nested lists.

```sema
(flatten-deep '(1 (2 (3 (4)))))   ; => (1 2 3 4)
```

### `zip`

Combine corresponding elements from two lists into pairs.

```sema
(zip '(1 2 3) '("a" "b" "c"))   ; => ((1 "a") (2 "b") (3 "c"))
```

### `partition`

Split a list into two lists based on a predicate. Returns a list of two lists: elements that satisfy the predicate and those that don't.

```sema
(partition even? '(1 2 3 4 5))   ; => ((2 4) (1 3 5))
```

## Searching

### `member`

Return the tail of the list starting from the first matching element.

```sema
(member 3 '(1 2 3 4))   ; => (3 4)
(member 9 '(1 2 3))     ; => #f
```

### `list/contains?`

Return `#t` if the list contains the element, else `#f`. Unlike `member` (which returns the Scheme-style tail or `#f`), this reads as a predicate.

```sema
(list/contains? '(1 2 3) 2)   ; => #t
(list/contains? '(1 2 3) 9)   ; => #f
```

### `list/nth-or`

Indexed access with a fallback: returns the element at `index`, or `default` when out of bounds (the safe counterpart to `nth`, which errors).

```sema
(list/nth-or '(10 20 30) 1 :none)   ; => 20
(list/nth-or '(10 20 30) 9 :none)   ; => :none
```

### `any`

Test if any element satisfies a predicate.

```sema
(any even? '(1 3 5 6))   ; => #t
(any even? '(1 3 5))     ; => #f
```

### `every`

Test if all elements satisfy a predicate.

```sema
(every even? '(2 4 6))     ; => #t
(every even? '(2 3 6))     ; => #f
```

### `list/index-of`

Return the index of the first occurrence of a value, or `nil` if not found.

```sema
(list/index-of '(10 20 30) 20)   ;; => 1
(list/index-of '(10 20 30) 99)   ;; => nil
```

### `list/unique`

Remove duplicate elements, preserving order.

```sema
(list/unique '(1 2 2 3 3 3))   ; => (1 2 3)
```

### `list/dedupe`

Remove consecutive duplicates from a list.

```sema
(list/dedupe '(1 1 2 2 3 3 2))   ; => (1 2 3 2)
```

## Grouping

### `list/group-by`

Group elements by a function, returning a map.

```sema
(list/group-by even? '(1 2 3 4 5))   ; => {#f (1 3 5) #t (2 4)}
```

### `list/interleave`

Interleave elements from two lists.

```sema
(list/interleave '(1 2 3) '(a b c))   ; => (1 a 2 b 3 c)
```

### `list/chunk`

Split a list into chunks of a given size.

```sema
(list/chunk 2 '(1 2 3 4 5))   ; => ((1 2) (3 4) (5))
(list/chunk 3 '(1 2 3 4 5 6)) ; => ((1 2 3) (4 5 6))
```

### `frequencies`

Count occurrences of each element, returning a map.

```sema
(frequencies '(a b a c b a))   ; => {a 3 b 2 c 1}
```

### `interpose`

Insert a separator between elements.

```sema
(interpose ", " '("a" "b" "c"))   ; => ("a" ", " "b" ", " "c")
```

## Aggregation

### `list/sum`

Sum all numbers in a list.

```sema
(list/sum '(1 2 3 4 5))   ; => 15
```

### `list/min`

Return the minimum value in a list.

```sema
(list/min '(3 1 4 1 5))   ; => 1
```

### `list/max`

Return the maximum value in a list.

```sema
(list/max '(3 1 4 1 5))   ; => 5
```

## Random

### `list/shuffle`

Return a randomly shuffled copy of a list.

```sema
(list/shuffle '(1 2 3 4 5))   ; => (3 1 5 2 4) (varies)
```

### `list/pick`

Pick a random element from a list.

```sema
(list/pick '(1 2 3 4 5))   ; => 3 (varies)
```

## Construction

### `list/repeat`

Create a list by repeating a value N times.

```sema
(list/repeat 3 0)   ; => (0 0 0)
(list/repeat 4 "x") ; => ("x" "x" "x" "x")
```

### `make-list`

Alias for `list/repeat`.

```sema
(make-list 3 0)   ; => (0 0 0)
```

### `iota`

Generate a list of numbers. `(iota count)`, `(iota count start)`, or `(iota count start step)`.

```sema
(iota 5)         ; => (0 1 2 3 4)
(iota 3 10)      ; => (10 11 12)
(iota 4 0 2)     ; => (0 2 4 6)
```

## Splitting

### `list/split-at`

Split a list at a given index, returning two lists.

```sema
(list/split-at '(1 2 3 4 5) 3)   ; => ((1 2 3) (4 5))
```

### `list/take-while`

Take elements from the front while a predicate holds.

```sema
(list/take-while (fn (x) (< x 4)) '(1 2 3 4 5))   ; => (1 2 3)
```

### `list/drop-while`

Drop elements from the front while a predicate holds.

```sema
(list/drop-while (fn (x) (< x 4)) '(1 2 3 4 5))   ; => (4 5)
```

## Filtering

### `list/reject`

Return elements that do NOT satisfy a predicate (inverse of `filter`).

```sema
(list/reject even? '(1 2 3 4 5))   ; => (1 3 5)
```

### `list/find`

Return the first element that satisfies a predicate, or `nil` if none found.

```sema
(list/find even? '(1 3 4 5 6))   ; => 4
(list/find even? '(1 3 5))       ; => nil
```

### `list/sole`

Return the single element matching a predicate. Errors if zero or more than one match.

```sema
(list/sole (fn (x) (> x 4)) '(1 2 3 4 5))   ; => 5
```

## Set Operations

### `list/diff`

Return elements in the first list that are not in the second list.

```sema
(list/diff '(1 2 3 4 5) '(3 4))   ; => (1 2 5)
```

### `list/intersect`

Return elements present in both lists.

```sema
(list/intersect '(1 2 3 4 5) '(3 4 6))   ; => (3 4)
```

### `list/duplicates`

Return values that appear more than once in a list.

```sema
(list/duplicates '(1 2 2 3 3 3 4))   ; => (2 3)
```

## Extraction

### `list/pluck`

Extract a specific key from each map in a list.

```sema
(define people (list {:name "Alice" :age 30} {:name "Bob" :age 25}))
(list/pluck :name people)   ; => ("Alice" "Bob")
```

### `list/key-by`

Transform a list of maps into a map keyed by a function result.

```sema
(list/key-by (fn (p) (get p :id)) people)   ; => map keyed by :id
```

## Statistics

### `list/avg`

Return the average of a numeric list.

```sema
(list/avg '(2 4 6))   ; => 4.0
```

### `list/median`

Return the statistical median.

```sema
(list/median '(3 1 2))     ; => 2.0
(list/median '(1 2 3 4))   ; => 2.5
```

### `list/mode`

Return the most frequent value. If tied, returns a list.

```sema
(list/mode '(1 2 2 3 3 3))   ; => 3
(list/mode '(1 1 2 2))       ; => (1 2)
```

## Windowing

### `list/sliding`

Create a sliding window over a list. Optional step parameter.

```sema
(list/sliding '(1 2 3 4 5) 2)     ; => ((1 2) (2 3) (3 4) (4 5))
(list/sliding '(1 2 3 4 5 6) 2 3) ; => ((1 2) (4 5))
```

### `list/page`

Paginate a list. `(list/page items page per-page)` — 1-indexed pages.

```sema
(list/page (range 20) 1 5)   ; => (0 1 2 3 4)
(list/page (range 20) 2 5)   ; => (5 6 7 8 9)
```

### `list/cross-join`

Cartesian product of two lists.

```sema
(list/cross-join '(1 2) '(3 4))   ; => ((1 3) (1 4) (2 3) (2 4))
```

## Padding & Joining

### `list/pad`

Pad a list to a target length with a fill value.

```sema
(list/pad '(1 2 3) 5 0)   ; => (1 2 3 0 0)
```

### `list/join`

Join list elements into a string. Optional final separator.

```sema
(list/join '(1 2 3) ", ")             ; => "1, 2, 3"
(list/join '(1 2 3) ", " " and ")     ; => "1, 2 and 3"
```

## Generation

### `list/times`

Generate a list by calling a function N times with the index (0-based).

```sema
(list/times 5 (fn (i) (* i i)))   ; => (0 1 4 9 16)
```

## Utility

### `tap`

Apply a side-effect function to a value, then return the original value.

```sema
(tap 42 (fn (x) (println x)))   ; prints 42, returns 42
```
