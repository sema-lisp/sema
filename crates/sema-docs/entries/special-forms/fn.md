---
name: "fn"
module: "special-forms"
syntax: "(fn (params ...) body ...) | (fn params body ...)"
---

Create an anonymous function. `fn` is an alias for `lambda` and behaves identically. It takes a parameter list (or a single rest-parameter symbol) and one or more body expressions, returning a callable function value that closes over its defining environment.

Parameters may be supplied as a list or vector of symbols. Rest arguments use dot notation `(x . rest)`. Destructuring patterns in parameter positions are automatically desugared into an internal `let*`. A single symbol as the parameter spec captures all arguments as a list.

Use `fn` or `lambda` according to your preferred style; both are recognized as the same special form by the evaluator.

```sema
((fn (x) (* x x)) 4)  ; => 16
```

```sema
(map (fn (n) (+ n 1)) '(1 2 3))  ; => (2 3 4)
```

```sema
((fn (first . rest) rest) 10 20 30)  ; => (20 30)
```

```sema
((fn {:keys [x y]} (+ x y)) {:x 1 :y 2})  ; => 3
```
