---
name: "async/rejected"
module: "concurrency"
section: "Promises"
params: [{ name: message, type: any }]
returns: "promise"
see_also: ["async/resolved", "async/rejected?", "async/await"]
---

```sema
(async/rejected message) → async-promise
```

Create an already-rejected promise carrying the error `message`. Awaiting it re-raises that message. The error-path counterpart to `async/resolved`, useful as a failed base case in code that returns promises.

```sema
(async/rejected? (async/rejected "boom"))  ; => #t
;; (await (async/rejected "boom"))  → raises: boom
```
