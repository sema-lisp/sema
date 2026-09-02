---
name: "web/user-agent"
module: "playground"
section: "Web-Only Functions"
syntax: "(web/user-agent)"
returns: "string"
see_also: ["web/user-agent-data", "sys/platform"]
---

Return the browser's `navigator.userAgent` string. Works in all browsers.

```sema
(web/user-agent)
; => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 ..."
```
