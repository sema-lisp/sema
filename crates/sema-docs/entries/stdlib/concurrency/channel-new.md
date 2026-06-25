---
name: "channel/new"
module: "concurrency"
section: "Channels"
params: [{ name: capacity, type: int, doc: "optional bound, defaults to 1; must be at least 1" }]
returns: "channel"
---

```sema
(channel/new)         → channel  ; capacity 1
(channel/new capacity) → channel
```

Create a bounded channel. Default capacity is 1. Capacity must be at least 1.
