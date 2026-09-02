---
name: "async/run"
module: "concurrency"
section: "Promises"
syntax: "(async/run)"
returns: "nil"
see_also: ["async/spawn", "async/await", "async/all"]
---

```sema
(async/run)
```

Drive the scheduler until every pending task has settled. Use it for fire-and-forget tasks you spawned but never `await` — without an `await` or an `async/run`, a spawned task just sits queued and never runs.

You rarely need this: `await` already drives the scheduler inline at the top level, so awaiting the last promise is enough in most scripts. Reach for `async/run` when tasks communicate through side effects (channels, mutation) rather than a returned promise.

```sema
(define out (channel/new 4))
(async (channel/send out :a))   ; spawned, not awaited
(async (channel/send out :b))
(async/run)                      ; let both tasks finish
(channel/count out)  ; => 2
```
