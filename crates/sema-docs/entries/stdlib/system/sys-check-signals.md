---
name: "sys/check-signals"
module: "system"
section: "Signals"
---

Dispatch any pending signal callbacks. Call this from your event loop (typically right after `io/read-key` / `io/read-key-timeout` returns) so handlers run in a predictable place rather than asynchronously interrupting Sema code.

```sema
(let loop ()
  (sys/check-signals)
  (let ((key (io/read-key-timeout 50)))
    (when key (handle-key key))
    (loop)))
```

If no signals are pending, this is essentially free — it just checks three atomic booleans.
