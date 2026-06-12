---
name: "channel/new"
module: "concurrency"
section: "Channels"
---

```sema
(channel/new)         → channel  ; capacity 1
(channel/new capacity) → channel
```

Create a bounded channel. Default capacity is 1. Capacity must be at least 1.
