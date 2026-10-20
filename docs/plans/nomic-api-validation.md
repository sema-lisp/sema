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

It also embeds eleven documents at 64 dimensions, crossing the ten-text HTTP
batch limit, and checks the aggregate count and nonzero token usage.

- [ ] Account has API access.
- [ ] Live test passes against the final release commit.
- [ ] Record commit, date, model, and test result in the PR (no credentials).
- [ ] Run the normal `jake ci` release checks on the combined changes.

Do not infer completion from passing mock tests or ordinary CI, which excludes
the ignored live test. No release version or tag is created by this work.

## Cross-language review (2026-10-21)

The review compared hosted Nomic integrations. Local Ollama and GPT4All examples
do not establish the hosted HTTP contract.

| Source | Corroboration | Difference that matters |
| --- | --- | --- |
| [Official Python SDK](https://github.com/nomic-ai/nomic/blob/b963f429ea40a40071257465253737db42ef6222/nomic/embed.py) | Endpoint, bearer auth, tasks, response fields, ordered vectors, usage totals | Caps each request at ten texts and explicitly defaults to `truncate`; Sema follows both |
| [LangChain JavaScript](https://github.com/langchain-ai/langchainjs/blob/1596213afe2874842e98d0827752b422096d020b/libs/providers/langchain-nomic/src/embeddings.ts) | Hosted request fields and document/query task selection | Its 400-item batching exceeds the current official SDK limit; do not copy it |
| [Go Scope](https://github.com/Tangerg/scope/blob/b7eaf3116728d4da0bf9de47ba1ac9b96d10d4f4/models/nomic/api.go) | Native endpoint and response schema, including model and both token counters | Uses synthetic fixtures too; these do not prove service access |
| [Java LangChain4j](https://github.com/langchain4j/langchain4j/blob/aff8ebadbca51602d010b4eb15bb08d56ab98f98/langchain4j-nomic/src/main/java/dev/langchain4j/model/nomic/NomicEmbeddingModel.java) | Hosted endpoint, task selection, batching | Its 500-item default is not the current official SDK limit |

[Nomic's batch-limit change](https://github.com/nomic-ai/nomic/pull/392) states
that ten texts would become the backend maximum. Sema sends sequential chunks
of at most ten and verifies vector widths across chunks. Tests cover ordering,
usage overflow, and later-chunk failures without returning partial results.

The [hosted embedding guide](https://docs.nomic.ai/atlas/embeddings-and-retrieval/generate-embeddings)
documents v1.5 dimensions from 64 to 768. Sema validates that model's range and
uses a 64-dimensional synthetic fixture. It does not apply the range to unknown
model overrides.

Remaining difference: the Python SDK and LangChain integrations retry transient
errors. Sema's embedding path does not use its chat retry loop. It returns HTTP
and rate-limit errors to the caller; the Rust rate-limit error retains the retry
hint. A shared embedding retry policy is separate work. Repeating a failed
multi-request call can charge again for successful earlier chunks.

Confidence is high in the small-request wire contract and deterministic local
behavior, but moderate in live operation until the release gate passes. None of
the source comparisons verifies account permissions, actual service defaults,
model quality, or current quota behavior for this account.
