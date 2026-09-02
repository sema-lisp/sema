---
name: "bytevector/set!"
module: "bytevectors"
section: "Access & Mutation"
params: [{ name: bv, type: bytevector }, { name: index, type: int }, { name: byte, type: int }]
returns: "bytevector"
see_also: ["bytevector-u8-set!", "bytevector/u8-set!", "bytevector/ref"]
---

Return a new bytevector with the byte at `index` replaced by `byte` (0..255).

Despite the `!`, this does **not** mutate in place — Sema bytevectors are copy-on-write, so the original is left untouched and you must keep the returned value. Out-of-range `index`, or a `byte` outside 0..255, signals an error.

```sema
(define bv #u8(1 2 3))
(bytevector/set! bv 0 9)   ; => #u8(9 2 3)
bv                         ; => #u8(1 2 3)  (original unchanged)
```

See also `bytevector/ref` for reading and `bytevector/u8-set!`, the byte-typed alias.
