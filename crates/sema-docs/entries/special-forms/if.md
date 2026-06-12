---
name: "if"
module: "special-forms"
syntax: "(if condition then-expr else-expr)"
---

Two-branch conditional special form. Evaluates `condition`, and if it is truthy evaluates `then-expr`; otherwise evaluates `else-expr`. Only the selected branch is evaluated — the other is never touched. In Sema, only `nil` and `#f` are falsy; every other value (including `0`, empty strings, and empty lists) is truthy.

The `else-expr` is optional. When omitted and the condition is falsy, `if` returns `nil`. Because `if` is a special form and not a function, it does not evaluate its arguments eagerly; this makes it suitable for guarding side effects or expensive computations.

```sema
(if (> x 0) "positive" "non-positive")
```

```sema
(if (empty? items)
  (println "nothing to do")
  (process items))
```

```sema
(define y 10)
(if (= y 0) "zero")   ; => nil (no else branch)
```

```sema
(if "hello" 1 2)      ; => 1 (non-empty string is truthy)
(if 0 1 2)            ; => 1 (zero is truthy)
(if nil 1 2)          ; => 2 (nil is falsy)
```
