---
name: "defpolicy"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(defpolicy name policy-map)"
returns: "map"
see_also: ["defworkflow", "step", "policy/without", "workflow/check"]
---

Define a reusable model and tool policy. Model rules match exact
`"provider/model"` identities or the `"provider/*"` wildcard. Tool rules can
allow or deny tool names and constrain model-supplied path, URL, and command
arguments. A present `:models` or `:tools` section defaults to `:deny`.

```sema
(defpolicy repository-auditor
  {:models {:default :deny
            :allow ["openai/gpt-5" "anthropic/*"]}
   :tools {:default :deny
           :allow
           {"read-file" {:paths ["src/**" "Cargo.toml"]}
            "run-command" {:commands ["cargo test" "cargo check"]}}}})

(defworkflow audit "Guarded audit" {:policy repository-auditor}
  (phase "Audit")
  (step "Inspect the repository."
        {:tools [read-file run-command]})
  {:status :success})
```

Attach a policy with workflow or step `:policy`. Active workflow and step
policies compose with logical AND. `:permissions` and the CLI sandbox remain the
outer capability limit.

Step policies may contain model, tool, subject, input, and output controls.
`:metadata` and `:completion` describe evidence for the whole run and must be
attached to the enclosing workflow. The runtime and `workflow check` reject
those sections on a step instead of silently ignoring them.

Policy denials raise a `:policy-denied` condition. The condition contains
`:message`, `:policy`, `:boundary`, `:subject`, `:rule`, `:reason`, `:action`,
and `:source`. This lets a `catch` handler use the exact deciding policy layer:

```sema
(try
  (llm/complete "Review this change.")
  (catch denial
    {:policy (:policy denial)
     :rule (:rule denial)
     :reason (:reason denial)}))
```

Invalid policy maps identify the invalid field or one-based list entry. Unknown
keys suggest a close valid key when possible. Invalid enum values list the
accepted keywords.

See also: `defworkflow`, `step`, `policy/without`, `workflow/check`.
