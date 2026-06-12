---
name: "i64-array/make"
module: "typed-arrays"
section: "Construction"
---

Create an i64 array of a given length, optionally filled with a value (default `0`).

```sema
(i64-array/make 5)     ; => #i64(0 0 0 0 0)
(i64-array/make 3 42)  ; => #i64(42 42 42)
```
