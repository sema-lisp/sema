---
name: "channel/count"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "int"
---

Return the number of items currently buffered in the channel. Errors if the argument is not a channel.

```sema
(define ch (channel/new 4))
(channel/send ch 1)
(channel/send ch 2)
(channel/count ch)  ; => 2
```
