---
name: "time/add"
module: "datetime"
section: "Arithmetic"
---

Add seconds to a timestamp. Returns a new timestamp. Use negative values to subtract.

```sema
(time/add timestamp seconds) ; => float (timestamp)
```

```sema
(define ts 1736943000.0)  ; 2025-01-15 12:10:00 UTC

(time/add ts 3600)       ; one hour later    => 1736946600.0
(time/add ts 86400)      ; one day later     => 1737029400.0
(time/add ts -3600)      ; one hour earlier  => 1736939400.0
(time/add ts (* 7 86400)) ; one week later
```

Common durations in seconds:

| Duration | Seconds |
|----------|---------|
| 1 minute | `60` |
| 1 hour | `3600` |
| 1 day | `86400` |
| 1 week | `604800` |
| 30 days | `2592000` |
