---
name: "gc/collect"
module: "system"
section: "Memory"
returns: "map"
---

Run a full cycle-collection pass now and return its stats as a map with `:candidates` (registered cycle candidates scanned), `:traced` (heap nodes visited), `:collected` (garbage nodes reclaimed), and `:pruned` (dead registry entries removed). Reference cycles (e.g. a recursive local closure capturing itself) are otherwise reclaimed automatically at safe points; call this to force a collection at a known-quiet moment or to observe reclamation directly. With OpenTelemetry tracing enabled, every collector pass (this one included) also emits a `gc.collect` span carrying the trigger and these stats as `gc.*` attributes.

```sema
(define (make-loop)
  (define (loop n) (if (<= n 0) 0 (loop (- n 1))))
  (loop 3))
(make-loop)
(gc/collect)   ; => {:candidates 3 :collected 4 :pruned 1 :traced 9}
```
