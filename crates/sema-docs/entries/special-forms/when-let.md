---
name: "when-let"
module: "special-forms"
syntax: "(when-let (var expr) body ...)"
---

`when-let` is a built-in macro that evaluates `expr`, binds the result to `var`, and evaluates the body only if the value is non-nil. If the value is `nil`, the entire form returns `nil` without evaluating the body. The binding is only visible inside the body expressions.

This form is ideal for conditional side effects or computations that depend on a value that might be absent.

```sema
(when-let (x (get {:a 1} :a)) (* x 10))
;; => 10
```

```sema
(when-let (x (get {:a 1} :b)) (* x 10))
;; => nil
```

```sema
(when-let (user (db/find-user id))
  (send-email user "Welcome back")
  (log-event "login" user))
;; => performs side effects only when a user is found
```

**Note:** `when-let` expands to a `let` and a `when`. It is defined in the prelude and does not require an import.
