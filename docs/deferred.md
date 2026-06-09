# Deferred items

Things that came out of the May 2026 quality sweep (Wave 6 audit) but were intentionally not fixed because they're too risky, too design-dependent, or have a cheap workaround. Each entry says *why* it's deferred so a future pass can decide whether to revisit.

Verified 2026-06-09: U6 ("did you mean" hints — shipped via `suggest_similar` in sema-core, attached in both backends) and U9 (REPL completeness check — replaced by the lexer-based `SemaValidator` in `crates/sema/src/repl/validator.rs`) were removed because they have since been fixed. Remaining entries re-verified as still open.

---

## D5 — Typed `try`/`catch` form

**Today:** `(try expr (catch e ...))` catches *every* error type, including `:unbound`, `:arity`, `:type-error` — the kind of errors that usually mean a typo. The docs (`website/docs/language/special-forms.md` near "Re-throw errors you don't intend to handle") explicitly warn about this.

**The bug shape:** silent bug-masking. A typo inside `try` is swallowed and the catch block runs as if the operation failed for "real" reasons.

**Proposed fix (not done):** add `(catch [:user :type-error] e ...)` syntax that filters by the `:type` field, mirroring Clojure's `catch ExceptionType` or Common Lisp's `handler-case`. Optionally lint-warn on the un-filtered form.

**Why deferred:** non-trivial language design. Affects reader (new pattern in catch clause), special-form lowering in both backends, and prelude macros that use `try`. Needs an ADR before code.

**Workaround today:** users can do `(try ... (catch e (if (= (:type e) :user) (handle e) (throw e))))` to re-raise unexpected errors. That's a documented pattern in special-forms.md.

---

## N5 — `server.rs` response-helper `.unwrap()`s

**Today:** `crates/sema-stdlib/src/server.rs` lines ~1028-1099 (as of 2026-06-09) unwrap on `as_map_rc()` / `__stream_handler` / `__ws_handler` after a single-marker `is_*_response` check. A user who constructs a partially-formed response map (sets `__file_path` flag but forgets `__stream_handler`) panics the HTTP server thread.

**Proposed fix:** convert each unwrap to `.ok_or_else(|| SemaError::eval("..."))?` and propagate via `Result<ServerResponse, SemaError>` — sending a `ServerResponse::Error` over the oneshot instead of panicking.

**Why deferred:** the helper functions return `()` today; restructuring to propagate errors via the existing `oneshot::Sender<ServerResponse>` requires a new `ServerResponse::Error` variant and changes to the axum-side handler. Medium-effort refactor with non-trivial blast radius.

**Workaround today:** users normally build response maps with `http/ok`, `http/file`, etc. — those constructors always produce well-formed maps. The bug only triggers if a user builds a map by hand with the wrong `__*` markers. Low-likelihood in practice.

---

## N7 — `sort` accepts heterogeneous types silently

**Today:** `(sort (list 1 "a" {:k 1}))` returns an order based on `Value`'s `Ord` impl (which depends on Spur indices and tag order). Reproducible within a process but not portable, and it's never what the user wanted.

**Proposed fix:** either (a) raise a type error when the input is heterogeneous, or (b) define a stable cross-type total order and document it.

**Why deferred:** design call. Strictness is the safer choice for users but breaks anyone relying on the current behavior; defining a stable order is a long-term spec commitment. Wants an ADR.

**Workaround today:** `(sort-by ...)` with an explicit comparator — works correctly across types because the user provides the comparator.

---

## L2 — Code-lens execution + `sema/evalResult` notification untested e2e

**Today:** `crates/sema-lsp/tests/e2e/test_code_lens.py` only verifies the lens command name; it never calls `workspace/executeCommand` with `sema.runTopLevel` and never listens for the `sema/evalResult` custom notification described in `website/docs/lsp.md:138-150`. A regression in the eval subprocess path or the notification payload would slip through.

**Proposed fix:** add a python e2e test that:
1. Sends `workspace/executeCommand { command: "sema.runTopLevel", arguments: [{uri, formIndex: 0}] }`
2. Waits on the client's incoming-notification queue for `method == "sema/evalResult"` with a small timeout
3. Asserts payload includes `ok`, `value`, `elapsedMs` fields

Pattern can mirror the diagnostic-waiting in `test_diagnostics.py`.

**Why deferred:** medium effort — the python test harness needs to handle async notifications cleanly, and the test depends on a subprocess `sema eval` running and returning. Not flaky-prone, just a lift to write right.

**Workaround today:** the unit-level path (the lens command itself) is tested. Integration regressions would surface during manual testing of the editor extension.

---

## VFS — clones on every read

**Today (updated 2026-06-09):** `vfs_read` returns `Option<Vec<u8>>`, cloning file contents on each call — the function now lives in `crates/sema-core/src/vfs.rs:15` (the embedded-binary VFS). The originally-cited `crates/sema-notebook/src/vfs.rs` has since become a different thing (disk-backed path-sandboxed shim) and is no longer relevant to this entry.

**Proposed fix:** return `Cow<'_, [u8]>` so cached reads can be borrowed, or back the VFS with `Arc<HashMap>` so the file table can hand out cheap reference-counted handles.

**Why deferred:** identified in PR #14 review (severity: medium). VFS read isn't a current hotspot — the notebook is interactive, not a high-throughput file server. Revisit if the notebook starts serving real bundles.

---

## A note on the truly long-term language design items

These are not deferred — they're design questions that need a deliberate decision before any code lands. They're tracked in `docs/wip.md` (the "Wave 6c" cluster), not here.
