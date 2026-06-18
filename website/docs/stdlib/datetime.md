---
outline: [2, 3]
---

# Date & Time

All timestamps in Sema are **UTC Unix timestamps** — the number of seconds since January 1, 1970 00:00:00 UTC. Timestamps are floating-point numbers with millisecond fractional precision.

::: tip
All `time/` functions operate in UTC. There is no timezone conversion support — if you need local time handling, compute the offset manually with `time/add`.
:::

## Current Time

### `time/now`

Return the current time as a UTC Unix timestamp in seconds, with fractional milliseconds.

```sema
(time/now)   ; => 1707955200.123
```

The integer part is seconds since the Unix epoch; the fractional part provides millisecond precision.

```sema
(define now (time/now))
(println "Current timestamp: " now)

;; Extract just the seconds (truncate fractional part)
(define whole-seconds (floor now))
```

### `time-ms`

Return the current time as Unix milliseconds (integer). Defined in the system module but useful alongside datetime operations.

```sema
(time-ms)   ; => 1707955200123
```

## Formatting

### `time/format`

Format a UTC Unix timestamp using a [strftime](#strftime-format-directives)-style format string.

```sema
(time/format timestamp format-string) ; => string
```

```sema
(define ts 1736943000.0)  ; 2025-01-15 12:10:00 UTC

(time/format ts "%Y-%m-%d")            ; => "2025-01-15"
(time/format ts "%H:%M:%S")            ; => "12:10:00"
(time/format ts "%Y-%m-%d %H:%M:%S")   ; => "2025-01-15 12:10:00"
(time/format ts "%A, %B %d, %Y")       ; => "Wednesday, January 15, 2025"
(time/format ts "%F")                  ; => "2025-01-15"  (shorthand for %Y-%m-%d)
(time/format ts "%T")                  ; => "12:10:00"    (shorthand for %H:%M:%S)
```

## Parsing

### `time/parse`

Parse a date string into a UTC Unix timestamp using a [strftime](#strftime-format-directives)-style format string. The input is treated as a **UTC naive datetime** — no timezone information is expected or applied.

```sema
(time/parse date-string format-string) ; => float (UTC timestamp)
```

```sema
(time/parse "2025-01-15 12:10:00" "%Y-%m-%d %H:%M:%S")    ; => 1736943000.0
(time/parse "2025-01-15 00:00:00" "%Y-%m-%d %H:%M:%S")    ; => 1736899200.0
(time/parse "15/01/2025 14:30:00" "%d/%m/%Y %H:%M:%S")    ; => 1736951400.0
```

::: info
The format string must provide enough directives to fully specify a date and time. Parsing a date-only string like `"%Y-%m-%d"` without time components will fail — always include time directives (e.g., `%H:%M:%S`).
:::

::: tip
The wall-clock time in the string is **always interpreted as UTC**, regardless of any offset present. `time/parse` does not apply timezone offsets — `"2025-01-15 12:10:00"` always yields the UTC timestamp for 12:10:00 UTC. To work with another timezone, convert the value to UTC yourself (subtract the offset) before parsing, then format/compute in UTC.
:::

**Roundtrip** — formatting a timestamp and parsing it back yields the original value:

```sema
(define ts 1700000000.0)
(define formatted (time/format ts "%Y-%m-%d %H:%M:%S"))
(define parsed (time/parse formatted "%Y-%m-%d %H:%M:%S"))
(= parsed ts)  ; => #t
```

::: warning
`time/parse` returns whole seconds — sub-second precision from the original timestamp is lost when roundtripping through format/parse.
:::

## Date Decomposition

### `time/date-parts`

Decompose a UTC Unix timestamp into a map of date/time components.

```sema
(time/date-parts timestamp) ; => map
```

```sema
(define ts 1736943000.0)  ; 2025-01-15 12:10:00 UTC
(define parts (time/date-parts ts))

(get parts :year)     ; => 2025
(get parts :month)    ; => 1
(get parts :day)      ; => 15
(get parts :hour)     ; => 12
(get parts :minute)   ; => 10
(get parts :second)   ; => 0
(get parts :weekday)  ; => "Wednesday"
```

The returned map contains these keys:

| Key | Type | Description | Example |
|-----|------|-------------|---------|
| `:year` | integer | Four-digit year | `2025` |
| `:month` | integer | Month (1–12) | `1` |
| `:day` | integer | Day of month (1–31) | `15` |
| `:hour` | integer | Hour (0–23) | `12` |
| `:minute` | integer | Minute (0–59) | `10` |
| `:second` | integer | Second (0–59) | `0` |
| `:weekday` | string | Full weekday name | `"Wednesday"` |

The `:weekday` value is the full English weekday name: `"Monday"`, `"Tuesday"`, `"Wednesday"`, `"Thursday"`, `"Friday"`, `"Saturday"`, `"Sunday"`.

## Arithmetic

### `time/add`

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

### `time/diff`

Compute the difference between two timestamps in seconds. Returns `t1 - t2` (the first argument minus the second). The result can be negative.

```sema
(time/diff t1 t2) ; => float (seconds)
```

```sema
(define morning 1736935800.0)   ; 2025-01-15 10:10:00 UTC
(define afternoon 1736943000.0) ; 2025-01-15 12:10:00 UTC

(time/diff afternoon morning)  ; =>  7200.0  (2 hours)
(time/diff morning afternoon)  ; => -7200.0  (negative — morning is earlier)
(time/diff morning morning)    ; =>  0.0
```

::: tip
`time/diff` returns a signed value: positive when `t1 > t2`, negative when `t1 < t2`. Use `abs` if you need the absolute elapsed time regardless of order.
:::

## Delay

### `sleep`

Pause execution for a given number of milliseconds. Returns `nil`.

```sema
(sleep milliseconds) ; => nil
```

```sema
(sleep 1000)  ; sleep for 1 second
(sleep 500)   ; sleep for 500ms
(sleep 0)     ; yield (no-op pause)
```

Note that `sleep` takes **milliseconds** (not seconds), unlike the `time/` functions which work in seconds.

## strftime Format Directives

The `time/format` and `time/parse` functions use [chrono strftime](https://docs.rs/chrono/latest/chrono/format/strftime/index.html) format directives. Here are the most common ones:

### Date

| Directive | Description | Example |
|-----------|-------------|---------|
| `%Y` | Four-digit year | `2025` |
| `%m` | Month (zero-padded, 01–12) | `01` |
| `%d` | Day of month (zero-padded, 01–31) | `15` |
| `%e` | Day of month (space-padded) | `15` |
| `%B` | Full month name | `January` |
| `%b` | Abbreviated month name | `Jan` |
| `%A` | Full weekday name | `Wednesday` |
| `%a` | Abbreviated weekday name | `Wed` |
| `%u` | Day of week (1=Monday, 7=Sunday) | `3` |
| `%j` | Day of year (001–366) | `015` |
| `%F` | ISO 8601 date (`%Y-%m-%d`) | `2025-01-15` |

### Time

| Directive | Description | Example |
|-----------|-------------|---------|
| `%H` | Hour, 24-hour (zero-padded, 00–23) | `12` |
| `%I` | Hour, 12-hour (zero-padded, 01–12) | `12` |
| `%M` | Minute (zero-padded, 00–59) | `10` |
| `%S` | Second (zero-padded, 00–59) | `00` |
| `%p` | AM/PM | `PM` |
| `%T` | Time (`%H:%M:%S`) | `12:10:00` |
| `%R` | Short time (`%H:%M`) | `12:10` |

### Combined & Special

| Directive | Description | Example |
|-----------|-------------|---------|
| `%c` | Locale date and time | `Wed Jan 15 12:10:00 2025` |
| `%s` | Unix timestamp (seconds) | `1736943000` |
| `%Z` | Timezone abbreviation | `UTC` |
| `%%` | Literal `%` | `%` |

## Common Patterns

### Measuring elapsed time

```sema
(define start (time/now))
;; ... do some work ...
(define end (time/now))
(define elapsed (time/diff end start))
(println (format "Took ~a seconds" elapsed))
```

### ISO 8601 formatting

```sema
(define ts (time/now))
(time/format ts "%Y-%m-%dT%H:%M:%SZ")  ; => "2025-01-15T12:10:00Z"
(time/format ts "%F")                    ; => "2025-01-15" (date only)
```

### Calculating "N days ago"

```sema
(define now (time/now))
(define one-week-ago (time/add now (* -7 86400)))
(define thirty-days-ago (time/add now (* -30 86400)))

(println "One week ago: " (time/format one-week-ago "%Y-%m-%d"))
```

### Formatting for display

```sema
(define ts (time/now))
(time/format ts "%A, %B %d, %Y")    ; => "Wednesday, January 15, 2025"
(time/format ts "%I:%M %p")          ; => "12:10 PM"
(time/format ts "%b %d at %H:%M")   ; => "Jan 15 at 12:10"
```

### Checking the day of the week

```sema
(define parts (time/date-parts (time/now)))
(define day (get parts :weekday))

(if (or (= day "Saturday") (= day "Sunday"))
  (println "It's the weekend!")
  (println "It's a weekday."))
```

### Computing duration between dates

```sema
(define start (time/parse "2025-01-01 00:00:00" "%Y-%m-%d %H:%M:%S"))
(define end   (time/parse "2025-03-15 00:00:00" "%Y-%m-%d %H:%M:%S"))

(define diff-seconds (time/diff end start))
(define diff-days (/ diff-seconds 86400))
(println (format "~a days between dates" diff-days))
```

## Edge Cases

### Unix epoch

```sema
(time/format 0.0 "%Y-%m-%d %H:%M:%S")  ; => "1970-01-01 00:00:00"
(time/date-parts 0.0)
; => {:day 1 :hour 0 :minute 0 :month 1 :second 0 :weekday "Thursday" :year 1970}
```

### Negative timestamps (dates before 1970)

```sema
(time/format -86400.0 "%Y-%m-%d")  ; => "1969-12-31"
(time/format -31536000.0 "%Y-%m-%d")  ; => "1969-01-01"
```

### Sub-second precision

`time/now` returns millisecond fractional precision. `time/add` and `time/diff` preserve fractional seconds. However, `time/parse` returns whole seconds only.

```sema
(define ts (time/add 1736943000.0 0.5))   ; add 500ms
(time/diff ts 1736943000.0)               ; => 0.5
```
