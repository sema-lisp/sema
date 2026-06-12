---
name: "async"
module: "special-forms"
syntax: "(async body ...)"
---

Spawn the body expressions as an asynchronous task and return a promise for its result. The body is wrapped in a zero-argument thunk and handed to `async/spawn`; it runs cooperatively on the VM scheduler when driven by `await`, `async/run`, or `async/all`. This form is **VM-only** — using the tree-walker backend (`--tw`) raises an error because the tree-walker does not implement the cooperative scheduler required for async tasks.

```sema
(define p (async (+ 1 2)))
(await p)   ; => 3
```

Multiple async tasks can run concurrently. They interleave at yield points such as channel operations, `await`, or `sleep`.

```sema
(let ((p1 (async (* 3 3)))
      (p2 (async (* 4 4))))
  (+ (await p1) (await p2)))   ; => 25
```

The promise returned by `async` can be passed around, stored in data structures, or awaited multiple times. If the body completes normally, the promise resolves to the last expression's value. If the body throws an error, the promise rejects and the error is re-raised when the promise is awaited.

```sema
(define slow (async
               (sleep 100)
               "done"))
(await slow)   ; => "done"
```

**Note:** Async features require the VM backend. The tree-walker returns an error if `async` is encountered.
