---
name: "channel/try-recv"
module: "concurrency"
section: "Channels"
---

```sema
(channel/try-recv ch) → value | nil
```

Non-blocking receive. Returns the next value or `nil` if the channel is empty.
