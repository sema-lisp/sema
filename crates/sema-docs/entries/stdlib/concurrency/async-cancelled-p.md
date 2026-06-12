---
name: "async/cancelled?"
module: "concurrency"
section: "Promises"
---

```sema
(async/cancelled? promise) → bool
```

`#t` if `promise` is in the `Cancelled` state — distinct from `async/rejected?`. Matches the state variant directly rather than the rejection message, so a user `(async/rejected "cancelled")` no longer aliases:

```sema
(async/cancelled? (async/rejected "cancelled"))  ;; => #f
```
