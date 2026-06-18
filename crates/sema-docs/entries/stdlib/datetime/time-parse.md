---
name: "time/parse"
module: "datetime"
section: "Parsing"
---

Parse a date string into a UTC Unix timestamp using a [strftime](#strftime-format-directives)-style format string. The input is treated as a **UTC naive datetime** — no timezone information is expected or applied.

```sema
(time/parse date-string format-string) ; => float (UTC timestamp)
```

```sema
(time/parse "2025-01-15 12:10:00" "%Y-%m-%d %H:%M:%S")    ; => 1736943000.0
(time/parse "2025-01-15 00:00:00" "%Y-%m-%d %H:%M:%S")    ; => 1736899200.0
(time/parse "15/01/2025 14:30:00" "%d/%m/%Y %H:%M:%S")    ; => 1736951400.0
```

The format string must provide enough directives to fully specify a date and time. Parsing a date-only string like `"%Y-%m-%d"` without time components will fail — always include time directives (e.g., `%H:%M:%S`).

The wall-clock time in the string is **always interpreted as UTC**, regardless of any offset present. `time/parse` does not apply timezone offsets. To work with another timezone, convert the value to UTC yourself (subtract the offset) before parsing, then format/compute in UTC.

**Roundtrip** — formatting a timestamp and parsing it back yields the original value:

```sema
(define ts 1700000000.0)
(define formatted (time/format ts "%Y-%m-%d %H:%M:%S"))
(define parsed (time/parse formatted "%Y-%m-%d %H:%M:%S"))
(= parsed ts)  ; => #t
```

`time/parse` returns whole seconds — sub-second precision from the original timestamp is lost when roundtripping through format/parse.
