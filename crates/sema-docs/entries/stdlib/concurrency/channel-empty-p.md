---
name: "channel/empty?"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "bool"
---

`#t` if the channel's buffer currently holds no items, `#f` otherwise. Errors if the argument is not a channel.

```sema
(channel/empty? (channel/new 1))  ; => #t
```
