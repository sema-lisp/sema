---
name: "async/cancelled?"
module: "concurrency"
section: "Promises"
params: [{ name: promise, type: promise }]
returns: "bool"
see_also: ["async/cancel", "async/rejected?", "async/resolved?", "async/pending?"]
---

```sema
(async/cancelled? promise) → bool
```

`#t` if `promise` is in the `Cancelled` state — distinct from `async/rejected?`. Matches the state variant directly rather than the rejection message, so a user `(async/rejected "cancelled")` no longer aliases:

```sema
(async/cancelled? (async/rejected "cancelled"))  ;; => #f
```

Reads the settled state only. `async/cancel` is a request: a task cancelled before it started settles immediately (`async/cancelled?` is `#t` right away), but a task parked on a wait settles only when the runtime tears the wait down, so this read can be `#f` immediately after a successful cancel. Await the promise (under `try`) first for a deterministic answer — see `async/cancel`.
