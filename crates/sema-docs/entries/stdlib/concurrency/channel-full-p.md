---
name: "channel/full?"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "bool"
---

`#t` if the channel's buffer has reached its capacity, `#f` otherwise. A further `channel/send` on a full channel yields (inside an async task) or errors (at top level). Errors if the argument is not a channel.

```sema
(define ch (channel/new 1))
(channel/send ch 1)
(channel/full? ch)  ; => #t
```
