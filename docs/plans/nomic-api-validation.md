# Nomic API release gate

Status: implementation and schema-based tests can be reviewed; **do not release
the Nomic adapter until the live smoke test passes**. Account approval is pending.
Synthetic fixtures establish the request/response contract, not service access.

## Sources

- [Nomic text embedding API](https://docs.nomic.ai/reference/api/embed-text-v-1-embedding-text-post)
  specifies bearer authentication, the native endpoint, request fields, and the
  required response fields. Checked 2026-10-20.
- [Official SDK at b963f429](https://github.com/nomic-ai/nomic/blob/b963f429ea40a40071257465253737db42ef6222/nomic/embed.py)
  corroborates tasks, endpoint, batch response shape, and usage accounting.
- Mock fixture provenance: `crates/sema/tests/fixtures/nomic/README.md`.

The public `RerankDialect::Nomic` variant is retained for existing Rust callers
using custom hosts. The hosted adapter does not select it: the reference has no
hosted reranking endpoint or documented `nomic-rerank-v1.5` model.

## Required live verification

After API access is approved, supply `NOMIC_API_KEY` through the environment and
run this explicit test. It fails if the key is missing; it must not silently skip.
Do not paste credentials into logs, commits, or the PR.

```sh
cargo test -p sema-lang --test llm_suite nomic_test::live_nomic_embedding_release_gate -- --ignored --exact
```

The test uses the real default endpoint, checks single and batch inputs,
document/query tasks, 768-dimensional default vectors, a 256-dimensional
override, long-text options, and spawned dispatch. It also checks that the query
is more similar to the relevant document. HTTP, schema, model, or permission
errors fail the test.

- [ ] Account has API access.
- [ ] Live test passes against the final release commit.
- [ ] Record commit, date, model, and test result in the PR (no credentials).
- [ ] Run the normal `jake ci` release checks on the combined changes.

Do not infer completion from passing mock tests or ordinary CI, which excludes
the ignored live test. No release version or tag is created by this work.
