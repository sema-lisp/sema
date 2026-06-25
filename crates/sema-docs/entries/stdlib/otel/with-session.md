---
name: "with-session"
module: "otel"
section: "Observability"
---

Macro: run the body with a session scope so every span started inside (including `llm/*` and `agent/*` calls) is grouped under the session id in session-aware backends (Langfuse Sessions/Users). The config map carries an optional `:user`; use `{}` for none. The ergonomic form of `(otel/with-session id config thunk)`. A no-op when telemetry is disabled.

```sema
(with-session "chat-42" {:user "alice"}
  (llm/complete "...")          ; grouped under session chat-42, user alice
  (my-custom-pipeline))
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
