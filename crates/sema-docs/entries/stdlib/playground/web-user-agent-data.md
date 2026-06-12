---
name: "web/user-agent-data"
module: "playground"
section: "Web-Only Functions"
---

Return structured browser information from `navigator.userAgentData`. Returns a map on Chromium-based browsers (Chrome, Edge, Opera), `nil` on Firefox and Safari.

```sema
(web/user-agent-data)
; Chromium => {:mobile false :platform "macOS" :brands ("Chromium/120" "Google Chrome/120")}
; Firefox/Safari => nil
```

`userAgentData` is the modern replacement for UA string parsing — it returns structured, reliable data instead of a messy string. However, it's Chromium-only. Use `web/user-agent` for cross-browser compatibility.
