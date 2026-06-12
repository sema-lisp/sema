---
name: "async/all"
module: "concurrency"
section: "Promises"
---

```sema
(async/all promises) → list
```

Run all promises to completion and return a list of their results. Takes a list or vector of promises.

```sema
(let ((p1 (async 10))
      (p2 (async 20))
      (p3 (async 30)))
  (async/all (list p1 p2 p3)))  ; => (10 20 30)
```
