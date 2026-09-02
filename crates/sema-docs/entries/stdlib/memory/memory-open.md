---
name: "memory/open"
module: "memory"
section: "Agent Memory"
syntax: "(memory/open {:id id :namespace ns})"
returns: "map"
see_also: ["memory/append", "memory/messages", "agent/run"]
---

Open (or return) a named memory thread: a persistable, append-only conversation log keyed
on `(:namespace, :id)`. If a thread with the same key is already open in this process it
is returned as-is — `memory/open` is idempotent within a run. Any turns written to disk by
a previous run are loaded immediately, so prior conversation turns are visible without a
separate reload step.

Returns a **handle** map `{:memory/id id :memory/namespace ns}` that all other `memory/*`
functions accept.

- `:id` — required string; identifies this memory thread (e.g. `"user-42"`,
  `"session-abc"`). Must be a plain name (no path separators).
- `:namespace` — optional string; groups threads (default `"default"`).

Persistence path: `./.sema/memory/<namespace>/<id>.jsonl` (project-local, beside
`workflow/run`'s `./.sema/runs/`).

```sema
(define mem (memory/open {:id "user-42" :namespace "support"}))

;; append a turn
(memory/append mem {:role "user" :content "Hello"})

;; use as conversation history in an agent call
(agent/run bot "Summarize the session." {:memory mem})
```

See also: `memory/append`, `memory/messages`, `agent/run`.
