---
name: "eval"
module: "special-forms"
syntax: "(eval expr)"
---

Evaluate a data structure as code at runtime. `eval` takes a single expression, evaluates it to obtain a Sema value (typically a quoted form or the result of `read`), then evaluates that value as Sema code in the current environment.

This is the core primitive for metaprogramming. It allows programs to construct or read code dynamically and then execute it. Because the evaluated code runs in the caller's environment, it can access and mutate existing bindings. `eval` is used internally by the REPL and by tools that transform and execute code at runtime.

```sema
(eval '(+ 1 2))                         ; => 3
```

```sema
(eval (read "(* 3 4)"))                 ; => 12
```

```sema
(define x 10)
(eval '(set! x 20))
x                                       ; => 20
```

```sema
(define ops '(+ - *))
(eval (list (car ops) 3 4))             ; => 7
```

**Note:** `eval` takes exactly one argument. Passing zero or more than one argument raises an arity error. The expression is evaluated in the current dynamic environment, so closures and module state are respected.
