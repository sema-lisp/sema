---
name: "progn"
module: "special-forms"
syntax: "(progn body ...)"
---

Alias for `begin`. Evaluates each expression in order and returns the value of the last one. All intermediate results are discarded, making it useful for grouping side effects into a single expression. The last expression is evaluated in tail position, so tail-call optimization applies.

`progn` exists for compatibility with Common Lisp and other Lisp dialects. It behaves identically to `begin` in every way, including returning `nil` when given no arguments. New code should generally prefer `begin`, but either form is fully supported.

```sema
(progn
  (println "side effect")
  (+ 1 2))            ; => 3
```

```sema
(define counter 0)
(progn
  (set! counter (+ counter 1))
  (set! counter (+ counter 1))
  counter)            ; => 2
```

```sema
(progn)               ; => nil
```

```sema
(let ((x 10))
  (progn
    (println x)
    (* x 2)))         ; => 20
```
