---
name: "delay"
module: "special-forms"
syntax: "(delay expr)"
---

Create a lazy promise. `delay` captures its body expression without evaluating it, returning a promise object (also called a thunk) that can be forced later. This is useful for deferred or expensive computations that may never be needed, or for building lazy streams and infinite sequences.

Internally, `delay` creates a parameterless closure that captures the current environment. The body is evaluated only when the promise is passed to `force`, and the result is memoized so subsequent calls to `force` return the cached value without re-executing the body. Use `promise?` to test whether a value is a promise, and `promise-forced?` to check whether it has already been evaluated.

```sema
(define p (delay (+ 1 2)))
p                                       ; => a promise object
(force p)                               ; => 3
```

```sema
(define counter 0)
(define p (delay (begin (set! counter (+ counter 1)) counter)))
(force p)                               ; => 1
(force p)                               ; => 1  (memoized, body not re-run)
counter                                 ; => 1
```

```sema
(promise? (delay 42))                   ; => #t
(promise? 42)                           ; => #f
```

```sema
(define p (delay 99))
(promise-forced? p)                     ; => #f
(force p)
(promise-forced? p)                     ; => #t
```
