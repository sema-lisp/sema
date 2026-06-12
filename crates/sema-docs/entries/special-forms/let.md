---
name: "let"
module: "special-forms"
syntax: "(let ((name value) ...) body ...)"
---

`let` binds local variables in parallel. All init expressions are evaluated in the outer environment before any bindings are created, so later bindings cannot refer to earlier ones. The result is the value of the last body expression.

`let` supports destructuring patterns in binding positions. You can use vector patterns like `[a b]` or map patterns like `{:keys [name]}` to extract values directly from the bound expression.

`let` also supports a named-let form for tail-recursive looping. When the first argument is a symbol instead of a bindings list, it names the loop and creates a self-referential function that can be called with new arguments.

```sema
(let ((x 10) (y 20))
  (+ x y))
;; => 30
```

```sema
(let (([a b c] '(1 2 3)))
  (+ a b c))
;; => 6
```

```sema
(let loop ((i 0) (sum 0))
  (if (= i 100)
    sum
    (loop (+ i 1) (+ sum i))))
;; => 4950
```

**Note:** Named `let` requires at least three arguments: the loop name, the bindings list, and one or more body expressions.
