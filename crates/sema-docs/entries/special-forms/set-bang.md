---
name: "set!"
module: "special-forms"
syntax: "(set! name value)"
---

Mutate an existing variable binding. `set!` evaluates its second argument and assigns the result to the binding named by the first argument, which must be a symbol. The binding must already exist in the current environment chain; `set!` cannot create new bindings.

If the symbol is not bound, `set!` raises an `:unbound` error. In interactive contexts, the error message may include a suggestion for a similar existing name when one is found in the environment.

Unlike functional rebinding with `let`, `set!` performs imperative mutation and is commonly used with `while` loops and other stateful algorithms. It always returns `nil`.

```sema
(define x 1)
(set! x 2)
x  ; => 2
```

```sema
(let ((total 0))
  (set! total (+ total 10))
  total)  ; => 10
```

```sema
(let ((n 0))
  (while (< n 3)
    (println n)
    (set! n (+ n 1)))
  n)  ; => 3
```
