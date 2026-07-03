---
name: "ws/recv"
module: "websocket"
section: "WebSocket Client"
params: [{ name: conn, type: connection }]
returns: "map"
---

Block until the next frame arrives, returning a tagged map for `match`: `{:text s}`, `{:binary bv}`, or `{:close {:code … :reason …}}`, and `nil` once the connection is drained. Native only — in the browser, receive via `ws/listen`.

```sema
(match (ws/recv sock)
  {:text t}   (println "text:" t)
  {:binary b} (handle-bytes b)
  {:close c}  (println "closed" (:code c))
  nil         (println "connection drained"))
```
