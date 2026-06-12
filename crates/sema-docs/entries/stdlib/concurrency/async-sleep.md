---
name: "async/sleep"
module: "concurrency"
section: "Promises"
---

```sema
(async/sleep ms)
```

Inside an async task, yield for at least `ms` milliseconds (real timing — the scheduler will not wake the task until the deadline elapses). Outside async, calls `thread::sleep`.
