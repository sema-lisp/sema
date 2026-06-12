---
name: "define"
module: "special-forms"
syntax: "(define name value) | (define (name params ...) body ...) | (define pattern value)"
---

Bind a value, function, or destructuring pattern in the current environment. `define` is the primary way to introduce names at the top level or inside a local scope. It always returns `nil`.

When the first argument is a symbol, the second argument is evaluated and bound to that name. When the first argument is a list starting with a symbol, `define` treats it as a function shorthand: `(define (f x) body)` is equivalent to `(define f (lambda (x) body))`. When the first argument is a vector or map pattern, the value is destructured and multiple bindings are created at once.

In interactive mode, redefining a builtin native function prints a warning to stderr.

```sema
(define x 42)
x  ; => 42
```

```sema
(define (square n) (* n n))
(square 7)  ; => 49
```

```sema
(define [a b c] '(1 2 3))
(+ a b c)  ; => 6
```

```sema
(define {:keys [host port]} {:host "localhost" :port 8080})
host  ; => "localhost"
port  ; => 8080
```
