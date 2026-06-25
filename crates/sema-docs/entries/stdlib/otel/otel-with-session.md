---
name: "otel/with-session"
module: "otel"
section: "Observability"
---

Run a thunk with a session scope: every span started inside (including `llm/*` and `agent/*` calls) is tagged with the session id, grouping the run in session-aware backends (Langfuse Sessions/Users). The optional config map carries `:user`. The previous scope is restored when the thunk returns. A no-op when telemetry is disabled. The `with-session` macro is the ergonomic form.

**Signature:** `(otel/with-session id thunk) → any` · `(otel/with-session id config thunk) → any`

```sema
(otel/with-session "chat-42" {:user "alice"}
  (fn ()
    (llm/complete "...")          ; grouped under session chat-42, user alice
    (my-custom-pipeline)))
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
