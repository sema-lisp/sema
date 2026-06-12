---
name: "letrec"
module: "special-forms"
syntax: "(letrec ((name value) ...) body ...)"
---

`letrec` binds local variables with mutual recursion. All names are created as placeholders first, then every init expression is evaluated in an environment where all names are already visible. This makes it possible for bindings to refer to each other, which is especially useful for mutually recursive functions.

`letrec` also supports destructuring patterns in binding positions. Because placeholders are initialized to `nil`, non-function init expressions that read other bindings will see the placeholder `nil`, not the final value. For this reason, `letrec` is most commonly used with function definitions.

The result is the value of the last body expression.

```sema
(letrec ((even? (fn (n) (if (= n 0) #t (odd? (- n 1)))))
         (odd?  (fn (n) (if (= n 0) #f (even? (- n 1))))))
  (even? 10))
;; => #t
```

```sema
(letrec ((fact (fn (n) (if (= n 0) 1 (* n (fact (- n 1)))))))
  (fact 5))
;; => 120
```

**Caution:** Reading another `letrec` binding before its init has finished will yield `nil`:

```sema
(letrec ((x y)
         (y 10))
  x)
;; => nil
```
