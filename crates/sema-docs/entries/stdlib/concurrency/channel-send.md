---
name: "channel/send"
module: "concurrency"
section: "Channels"
---

```sema
(channel/send ch value)
```

Send a value to the channel. If the channel is full and inside an async task, yields until space is available. Outside async context, raises an error if full. Raises an error if the channel is closed.
