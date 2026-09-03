---
name: "fn"
module: "special-forms"
syntax: "(fn (params ...) body ...) | (fn [params ...] body ...)"
---

Create an anonymous function. `fn` is an alias for `lambda` and behaves identically. It takes a list or vector parameter specification and one or more body expressions, returning a callable function value that closes over its defining environment.

Parameters may be supplied as a list or vector of symbols. Rest arguments use dot notation `(x . rest)`. A destructuring pattern may appear *in place of a parameter symbol* (e.g. `(fn ([a b]) ...)`); it is desugared into an internal `let*`. A bare symbol as the entire parameter specification is not supported; use a dotted parameter list when the function needs rest arguments.

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

A parameter can be a destructuring pattern — a vector pattern over a list, or a map pattern over a map. The pattern stands in for one positional argument, so it must be wrapped in the parameter list:

```sema
((fn ([a b]) (+ a b)) '(3 4))            ; => 7
((fn ({:keys [x y]}) (+ x y)) {:x 1 :y 2})  ; => 3
```
