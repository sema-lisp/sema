---
name: "force"
module: "special-forms"
syntax: "(force promise)"
---

Evaluate a delayed promise created by `delay` and return its value. If the promise has already been forced, `force` returns the cached (memoized) result without re-evaluating the body. This makes `force` safe to call multiple times on the same promise.

`force` evaluates its argument first, then checks whether the result is a promise. If so, it evaluates the promise's body in the captured environment, stores the result, and returns it. If the argument is not a promise, both the tree-walker and VM backends raise a type error. This strict behavior prevents silently passing non-promise values through, which helps catch bugs early.

```sema
(define p (delay (+ 1 2)))
(force p)                               ; => 3
(force p)                               ; => 3  (cached)
```

```sema
(define counter 0)
(define p (delay (begin (set! counter (+ counter 1)) counter)))
(force p)                               ; => 1
(force p)                               ; => 1  (body only ran once)
```

```sema
(force 42)                              ; => error: expected thunk, got int
(force "hello")                         ; => error: expected thunk, got string
```

**Note:** Use `promise?` to test whether a value is a promise before calling `force`, and `promise-forced?` to check whether a promise has already been evaluated. These predicates are available in the standard library.
