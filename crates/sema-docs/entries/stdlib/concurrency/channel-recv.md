---
name: "channel/recv"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }]
returns: "any"
see_also: ["channel/try-recv", "channel/send", "channel/close"]
---

```sema
(channel/recv ch) → value
```

Receive the next value from the channel (FIFO). If the channel is **empty**: inside an async task `recv` *yields* until a sender provides a value; at the top level it raises (there is no scheduler to wait on — use `channel/try-recv` for a non-blocking peek). If the channel is **closed and drained**, `recv` returns `nil` — this is the standard "stream is done" signal a consumer loops on.

Because closed-and-empty yields `nil`, a `nil` from `recv` is ambiguous if you also send `nil` values; send a sentinel (e.g. a keyword) instead of `nil` when the stream may legitimately carry nils.

```sema
;; Drain a channel until the producer closes it.
(define ch (channel/new 4))
(channel/send ch 1)
(channel/send ch 2)
(channel/close ch)
(list (channel/recv ch)    ; => 1
      (channel/recv ch)    ; => 2
      (channel/recv ch))   ; => nil  (closed and empty)
```
