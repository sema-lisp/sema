---
name: "async/await"
module: "concurrency"
section: "Promises"
---

```sema
(async/await promise) → value
```

Wait for a promise to resolve. Inside an async task, yields to the scheduler. At the top level, runs the scheduler inline until the promise resolves. Raises an error if the promise was rejected.
