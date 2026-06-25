---
name: "channel/recv"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "any"
---

```sema
(channel/recv ch) → value
```

Receive a value from the channel. If the channel is empty and inside an async task, yields until data is available. Outside async context, raises an error if empty. Returns `nil` if the channel is closed and empty.
