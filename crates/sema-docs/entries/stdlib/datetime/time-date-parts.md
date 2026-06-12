---
name: "time/date-parts"
module: "datetime"
section: "Date Decomposition"
---

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
