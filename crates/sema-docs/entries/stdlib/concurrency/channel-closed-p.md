---
name: "channel/closed?"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "bool"
---

`#t` if the channel has been closed with `channel/close`, `#f` otherwise. Errors if the argument is not a channel.

```sema
(define ch (channel/new 1))
(channel/close ch)
(channel/closed? ch)  ; => #t
```
