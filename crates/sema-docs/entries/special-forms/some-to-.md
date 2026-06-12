---
name: "some->"
module: "special-forms"
syntax: "(some-> val form ...)"
---

Nil-safe thread-first macro. Threads `val` through a sequence of forms, inserting it as the first argument of each form, but short-circuits and returns `nil` as soon as any intermediate step produces `nil`. This is useful for safely navigating nested data structures or chaining operations where any step might legitimately return `nil`.

`some->` is a built-in macro defined in the prelude. It expands to a nested `let` and `if` expression using an auto-gensym temporary variable to avoid variable capture. If a form is a list, `val` is inserted after the function position; if a form is a bare symbol, it is treated as a function call with `val` as its sole argument.

```sema
(some-> {:a {:b 1}} (get :a) (get :b))   ; => 1
(some-> {:a {:b 1}} (get :x) (get :b))   ; => nil
```

```sema
(some-> config :database :connection-string db/connect)
;; returns nil if any step is nil, instead of crashing
```

```sema
(some-> 5 (+ 3) (* 2))                    ; => 16
(some-> nil (+ 3) (* 2))                  ; => nil
```

**Note:** Unlike `->`, which would attempt to call the next form with `nil` and likely raise a type or arity error, `some->` safely aborts the pipeline. Use `->` when you expect every step to succeed, and `some->` when `nil` is a valid intermediate result.
