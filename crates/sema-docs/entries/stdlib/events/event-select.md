---
name: "event/select"
module: "events"
section: "Events"
params: [{ name: sources, type: list, doc: "list of source maps" }, { name: timeout-ms, type: int, doc: "optional; defaults to the smallest timer among the sources, else 10000" }]
returns: "map or nil"
see_also: ["time/tick", "io/read-key-timeout", "proc/spawn", "sys/check-signals"]
---

Poll a list of event sources and return the first that becomes ready, or `nil` on timeout. `sources` is a list of maps: `{:type :key}` is ready when a keypress is waiting on stdin, `{:type :proc :handle h}` is ready when a `proc/spawn` handle has output or has exited, and `{:type :timer :ms n}` (see `time/tick`) is ready after `n` milliseconds. The optional `timeout-ms` bounds the wait; without it the smallest timer interval is used, and with no timers the wait is 10 seconds. Sources are scanned in order every few milliseconds, so this is a poll, not an edge-triggered wait, which is enough for a human-paced TUI loop.

The result is an event map with `:type` set to the ready source's type (`:key`, `:proc`, or `:timer`) and `:source` set to the source map that fired; a `:key` event also carries the key that was read under `:value`, and a `:proc` event carries `:output?` and `:exited?` booleans.

```sema
(event/select (list (time/tick 10)))       ; => {:source {:ms 10 :type :timer} :type :timer}
(event/select (list (time/tick 50)) 10)    ; => nil
```
