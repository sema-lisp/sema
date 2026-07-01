---
name: "mcp/close"
module: "mcp"
section: "MCP Client"
params: [{ name: handle, type: string, doc: "connection handle from mcp/connect" }]
returns: "nil"
---

```
(mcp/close handle)
```

Close an MCP connection. For a stdio server this terminates the child process;
for an HTTP server it best-effort ends the session (`DELETE`). After closing, the
handle is no longer valid.

Always call `mcp/close` when you're done: a connection stays alive (and a stdio
child keeps running) until you close it or the Sema process exits — the handle is
just an opaque string, so letting it go out of scope does **not** disconnect.

```sema
(define fs (mcp/connect {:command "npx" :args ["-y" "server-filesystem" "/tmp"]}))
;; … use it …
(mcp/close fs)
```
