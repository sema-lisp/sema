---
name: "otel/tool-span"
module: "otel"
section: "Observability"
---

Run a thunk inside a typed **TOOL** span for a tool you invoke yourself. Sets `gen_ai.operation.name` = `execute_tool`, `gen_ai.tool.name` = `name`, and (when `SEMA_OTEL_COMPAT` is set) the backend-native TOOL span-kind, so it classifies like the built-in tool spans. Optional third arg is an attributes map. A no-op when telemetry is disabled.

**Signature:** `(otel/tool-span name thunk) → any` · `(otel/tool-span name thunk attrs) → any`

```sema
(otel/tool-span "lookup-weather"
  (fn () (weather "Oslo"))
  {:call-id "c-1"})
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
