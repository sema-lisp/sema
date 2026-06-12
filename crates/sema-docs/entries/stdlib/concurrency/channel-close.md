---
name: "channel/close"
module: "concurrency"
section: "Channels"
---

```sema
(channel/close ch)
```

Close the channel. Subsequent sends will error. Blocked receivers will wake with `nil`.
