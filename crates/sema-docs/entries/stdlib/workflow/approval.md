---
name: "approval"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(approval key {:reason string :subject value [:preview string]})"
returns: "bool"
see_also: ["workflow/approval", "defworkflow", "checkpoint", "workflow/run"]
---

Stop a workflow at a durable, host-controlled human approval gate. `key` is a keyword or string. `:reason` explains why approval is required, `:subject` identifies the exact action and is stored only as a SHA-256 digest, and optional `:preview` is operator-safe text that may be written to the request sidecar and shown in a prompt. `:reason` and `:preview` are each limited to 1024 characters.

```sema
(approval :release-signoff
  {:reason "Publish the release"
   :subject {:kind :external-action
             :target "pkg.sema-lang.com"
             :digest package-digest}
   :preview "Publish sema-policies@1.0.0"})
```

With no decision, the run ends `{:status :needs-approval …}` before later forms execute. `sema workflow run` prompts on a terminal by default. For a durable headless pause, create an approval key pair, pass the public-key file to `run`, then use the private-key file only with the separate `approve` or `reject` command. Decisions are Ed25519-signed and bound to the run, complete static import/package dependency closure, arguments, phase, key, occurrence, subject digest, request timestamp, and authority key. Imports and loads execute from the exact snapshotted bytes; files outside the preflight closure fail closed.

Approval is a sequential workflow gate. Call `approval` directly; `workflow/approval` cannot be aliased, stored, or passed as a first-class value. Put the gate before `parallel`, `pipeline`, async task combinators, steps, `try` and `guard` handlers, retry/timeout wrappers, resource-cleanup forms, or a nested workflow; the static checker rejects gates inside those constructs. The subject must be canonical immutable data (scalars, lists/vectors, maps, bytevectors, or typed numeric arrays). Pending, rejected, malformed, and authority-invalid gates cannot be bypassed with Sema `try`/`catch`.

The form takes two arguments: the gate `key` (a keyword or string) and an options map with the required `:reason` (string) and `:subject` (canonical immutable data) keys and the optional `:preview` (string) key. When a valid approved decision exists for this run, key, occurrence, and subject digest, the form returns `#t` and the workflow continues. When no decision exists, the run stops with `{:status :needs-approval :approval-id "..."}`; a rejected decision stops it with `{:status :rejected :approval-id "..."}`; a malformed or wrongly signed decision also stops the run. The form raises an error when called outside `workflow/run`, when `:reason` or `:subject` is missing, when `:reason` or `:preview` exceeds 1024 characters, or when the subject is not canonical data (for example a function or a mutable array). The example below runs to `:needs-approval` on the first `sema workflow run` and completes after `sema workflow approve` records the decision.

```sema
(defworkflow publish
  "Publish a package after a human signs off."
  {:phases ["Build" "Publish"]}

  (phase "Build")
  (def digest (checkpoint :digest (step "Build the package and return its digest." {:schema :string})))

  (phase "Publish")
  (approval :release-signoff
    {:reason "Publish the release"
     :subject {:kind :external-action :target "pkg.sema-lang.com" :digest digest}
     :preview (str "Publish package " digest)})
  (step "Publish the package.")
  {:status :success})
```

See also: `workflow/approval`, `defworkflow`, `checkpoint`, `workflow/run`.
