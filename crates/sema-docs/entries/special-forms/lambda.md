---
name: "lambda"
module: "special-forms"
syntax: "(lambda (params ...) body ...) | (lambda params body ...)"
---

Create an anonymous function. `lambda` takes a parameter list (or a single rest-parameter symbol) and one or more body expressions, returning a callable function value that closes over the environment where it was defined.

The parameter list may be a list or vector of symbols. Rest arguments are supported with dot notation: `(lambda (x . rest) rest)`. Destructuring patterns in parameter positions are automatically desugared into a `let*` inside the function body. When given a single symbol instead of a list, that symbol captures all arguments as a list.

The alias `fn` is accepted as an alternative spelling (Clojure-style). Both are handled identically by the evaluator.

```sema
((lambda (x y) (+ x y)) 3 4)  ; => 7
```

```sema
(define square (lambda (x) (* x x)))
(square 5)  ; => 25
```

```sema
((lambda (x . rest) rest) 1 2 3)  ; => (2 3)
```

```sema
((lambda [a b] (+ a b)) '(1 2))  ; => 3
```
