---
name: "workflow/policy-without"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(workflow/policy-without reason thunk)"
returns: "any"
see_also: ["policy/without", "defpolicy", "workflow/run"]
---

Run `thunk` with active model and tool policies bypassed, and emit a
`policy.bypassed` event for each protected boundary. `reason` must be a
non-empty string of at most 256 characters. This low-level thunk form requires
an active workflow policy.

Use the `policy/without` macro in application code. It requires a literal
reason and accepts ordinary body forms:

```sema
(policy/without "read the migration fixture"
  (step "Inspect the legacy fixture." {:tools [read-file]}))
```

The bypass does not change workflow `:permissions`, the CLI sandbox, or
allowed-path limits.

See also: `policy/without`, `defpolicy`, `workflow/run`.
