---
name: "async/resolved"
module: "concurrency"
section: "Promises"
params: [{ name: value, type: any }]
returns: "promise"
see_also: ["async/rejected", "async/resolved?", "async/await"]
---

```sema
(async/resolved value) → async-promise
```

Create an already-resolved promise wrapping `value`. Awaiting it returns `value` immediately with no scheduling. Handy as a base case when an interface expects a promise but the result is already in hand.

```sema
(await (async/resolved 42))  ; => 42
```
