---
name: "channel/send"
module: "concurrency"
section: "Channels"
params: [{ name: ch, type: channel }, { name: value, type: any }]
returns: "nil"
see_also: ["channel/recv", "channel/full?", "channel/close"]
---

```sema
(channel/send ch value)
```

Send a value into the channel. If the buffer has room, the value is queued and `send` returns immediately. If the channel is **full**, behavior depends on where you are: inside an async task `send` *yields* until a receiver frees a slot (back-pressure); at the top level (no scheduler to yield to) it raises rather than block the whole program. Sending on a **closed** channel always raises and the value is dropped.

```sema
;; Inside a task a full channel suspends the sender until the consumer catches up.
;; Capacity 1, two sends, two receives — the second send yields until the first
;; value is taken.
(define ch (channel/new 1))
(define producer
  (async (channel/send ch :a)    ; fits
         (channel/send ch :b)))  ; full → yields here
(await (async (list (channel/recv ch) (channel/recv ch))))  ; => (:a :b)
```
