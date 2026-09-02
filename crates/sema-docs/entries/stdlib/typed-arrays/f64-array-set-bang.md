---
name: "f64-array/set!"
module: "typed-arrays"
section: "Access & Mutation"
aliases: ["i64-array/set!"]
params: [{ name: arr, type: "f64-array | i64-array" }, { name: index, type: int }, { name: value, type: number }]
returns: "f64-array | i64-array"
see_also: ["f64-array/ref", "f64-array/make"]
---

Set the element at a given index, returning the (possibly new) array. The store is copy-on-write: it mutates the backing buffer in place only when this is the array's *sole* reference; otherwise it copies first so other holders of the same array don't see the change.

```sema
(f64-array/set! (f64-array 1.0 2.0 3.0) 1 9.9)  ; => #f64(1 9.9 3)
(i64-array/set! (i64-array 10 20 30) 2 99)       ; => #i64(10 20 99)
```

**Gotcha — always use the return value.** Despite the `!`, this is not an unconditional in-place store. When another binding still references the array, the original is left untouched and you get a fresh copy back. Treat it like an update that returns the new array:

```sema
(define a (f64-array 1.0 2.0 3.0))
(define b (f64-array/set! a 1 9.9))
a   ; => #f64(1 2 3)    ; `a` is shared, so it was copied, not mutated
b   ; => #f64(1 9.9 3)  ; the change lives in the returned array
```

So rebind (`(set! a (f64-array/set! a i v))`) or thread the result through a loop rather than relying on the mutation being visible through an existing variable.
