---
name: "case"
module: "special-forms"
syntax: "(case expr ((datum ...) body ...) ... [(else body ...)])"
---

Dispatch on a value by comparing it against literal datums. `case` evaluates `expr` once, then checks each clause in order. The first clause whose datum list contains the value is selected, and its body is evaluated. An optional `else` clause at the end acts as a catch-all. If no clause matches and there is no `else`, `case` returns `nil`.

Unlike `cond`, which evaluates arbitrary test expressions, `case` uses equality comparison against literal values. This makes it ideal for switching on keywords, numbers, or other constants. The datums in each clause are grouped in a sublist, so a single clause can match multiple values.

```sema
(case (:status response)
  ((:ok) "success")
  ((:error :timeout) "failure")
  (else "unknown"))
```

```sema
(case n
  ((1) "one")
  ((2 3) "two or three")
  (else "other"))
```

```sema
(case '(:a 1)
  ((:a) "alpha")
  ((:b) "beta")
  (else "unknown"))
;; => "alpha"
```

**Note:** The VM backend lowers `case` to a `let` binding for the key followed by nested `if` expressions, avoiding repeated evaluation of the discriminant.