---
name: "if-let"
module: "special-forms"
syntax: "(if-let (var expr) then else)"
---

`if-let` is a built-in macro that evaluates `expr`, binds the result to `var`, and chooses a branch based on whether the value is `nil`. If the value is non-nil, `then` is evaluated with `var` in scope; otherwise `else` is evaluated. The binding is only visible inside the `then` and `else` expressions.

This form is useful when you want to both test for the presence of a value and use it, without repeating the lookup or computation.

```sema
(if-let (x (get {:a 1} :a)) x "missing")
;; => 1
```

```sema
(if-let (x (get {:a 1} :b)) x "missing")
;; => "missing"
```

```sema
(if-let (user (db/find-user id))
  (greet user)
  "guest")
;; => greets the user, or falls back to "guest"
```

**Note:** `if-let` expands to a `let` and an `if`. It is defined in the prelude and does not require an import.
