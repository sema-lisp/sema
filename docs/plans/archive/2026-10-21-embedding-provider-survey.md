# Embedding provider survey and proposed implementation scope

Research date: 2026-10-21 (Europe/Oslo). This completed survey preserves the
proposal and evidence available on that date. Nomic release verification is
recorded in [the validation plan](nomic-api-validation.md).

Implementation record (2026-10-28): PRs #172, #173, and #174 were merged for
Sema 2.0. All six proposed presets are implemented and covered by deterministic
contract tests. Live gates pass on the release commit for Nomic, OpenRouter,
DeepInfra, Azure, and Cloudflare. Alibaba and TEI were not tested live. The
survey below preserves the research context and future-provider proposals;
current configuration and defaults are documented on the website.

## Recommendation

First centralize provider registration and make request dialects explicit.
Then add OpenRouter, DeepInfra, Azure OpenAI v1, Cloudflare Workers AI, and
Alibaba Model Studio as named dense-text presets. Add a tested Hugging Face
Text Embeddings Inference (TEI) configuration example for self-hosting.
These cover an aggregator, open-model hosting, enterprise Azure deployments,
Cloudflare accounts, regional Alibaba deployments, and self-hosted inference.

Several of these endpoints should already work through Sema's custom
OpenAI-compatible `base-url`. Named presets mainly add environment configuration,
defaults, documentation, and contract tests. Documentation compatibility is not
a live compatibility result. Test each chosen model and endpoint before claiming
it as verified.

Also expand explicit model tests for existing Jina, Voyage, Cohere, and Gemini
adapters. Keep existing defaults unchanged: replacing a model can invalidate
stored vectors even when the output dimensions are identical.

## Provider matrix

Effort is relative to the current implementation in PR #172. "Small" means a
preset, protocol tests, and documentation after the proposed refactor; it does
not mean that all optional features work without changes. API-host adoption
counts are generally unavailable. Model adoption evidence appears separately
below and must not be presented as a host's market share.

| Provider or deployment | What it adds | Dense-text integration and limits | Proposed priority |
| --- | --- | --- | --- |
| [OpenRouter](https://openrouter.ai/docs/api_reference/embeddings) | Multiple model vendors behind one account; its public catalog returned 33 embedding models | Small: bearer `/api/v1/embeddings`; select a model explicitly. Catalog availability does not establish identical limits across routed backends | This round |
| [DeepInfra](https://github.com/deepinfra/docs/blob/main/api-reference/introduction.mdx) | Another hosted route to open models, including Qwen-family embeddings | Small: bearer `/v1/openai/embeddings`; verify selected model parameters | This round |
| [Azure OpenAI v1](https://learn.microsoft.com/en-us/azure/foundry/openai/api-version-lifecycle) | Azure resource/deployment integration | Small: resource-specific `/openai/v1` base URL and deployment name. The current v1 API accepts bearer API-key authentication; do not impose the older dated API's header/URL requirements | This round |
| [Cloudflare Workers AI](https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/) | BGE models in an existing Cloudflare account | Small: account-specific `/client/v4/accounts/{account}/ai/v1` base URL and bearer token | This round |
| [Alibaba Model Studio](https://www.alibabacloud.com/help/en/model-studio/text-embedding-synchronous-api) | Regional deployments and multilingual Qwen-family models | Small for dense vectors: regional/workspace OpenAI-compatible endpoint. `text-embedding-v4` has a 10-item batch limit; limits vary by model. Native sparse output requires separate types | This round, with an explicit region/workspace URL |
| [Hugging Face TEI](https://huggingface.co/docs/text-embeddings-inference/quick_tour) | Self-hosted models and control over data placement | Small for its OpenAI-compatible dense-text API: document and test a supported model/server revision. Pooling, instructions, normalization, and deployment settings remain significant | This round as a deployment recipe |
| [Nebius Token Factory](https://api.studio.nebius.com/openapi.json) | Another open-model hosting option; embedding and reranking endpoints | Small: bearer `/v1/embeddings`; model quality belongs to the selected model, not the host | Next, when its access/region/cost is useful |
| [SiliconFlow](https://docs.siliconflow.cn/docs/api/embeddings-post) | Another route to Qwen/BGE models | Small for dense text; verify regional account, endpoint, model IDs, and option support | Next, when its access/region/cost is useful |
| [NVIDIA NIM](https://docs.nvidia.com/nim/nemo-retriever/text-embedding/latest/reference.html) | Self-hosted NVIDIA inference and hosted model endpoints | Small to medium: synchronous endpoints resemble OpenAI, but truncation uses enums on some models. Some hosted APIs return HTTP 202 and require polling | Add a specific synchronous deployment first; avoid a blanket compatibility claim |
| [Amazon Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-titan-embed-text.html) | Titan/Cohere models under AWS deployment controls | Medium: model-specific `InvokeModel` bodies and responses. Titan takes one input per invocation. [Bedrock bearer API keys](https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys.html) are supported; full IAM credential discovery/refresh is additional work | Separate native adapter after compatible presets |
| [Vertex AI](https://cloud.google.com/vertex-ai/generative-ai/docs/model-reference/text-embeddings-api) | Google Cloud project/region/IAM integration | Medium to large: `:predict`, native instances/parameters, different response and usage fields, and credential integration. Do not assume Gemini developer-API compatibility | Separate native adapter when deployment demand is established |
| [Hugging Face Inference Providers](https://huggingface.co/docs/inference-providers/tasks/feature-extraction) | Managed feature extraction through supported backends | Medium: feature-extraction API and array responses; chat compatibility does not prove embedding compatibility | Prefer TEI first; add a tested native route if needed |
| [Mixedbread](https://www.mixedbread.com/docs/legacy-models) | Managed search and open embedding models | Direct embedding/rerank APIs are documented as legacy. Wholembed v3 is offered through managed search with late interaction | Defer hosted adapter until a current standalone contract is confirmed; open models remain candidates for self-hosting |
| [IBM watsonx](https://www.ibm.com/watsonx/developer/capabilities/embeddings/) | IBM enterprise deployments and supported Slate/Granite models | Medium: service-specific authentication, project, model, request, and response fields | Demand-driven follow-up |
| [Snowflake Cortex](https://docs.snowflake.com/en/sql-reference/functions/ai_embed) | Embeddings near Snowflake data, including Arctic models | Medium to large: SQL/service integration, not an OpenAI preset | Demand-driven follow-up |

Contextual AI's public platform material establishes managed RAG, but this survey
did not verify a standalone public embedding API contract. Novita appeared in
discovery sources, but a current primary embedding contract was not verified.
Neither is ready for an implementation commitment on this evidence.

## Existing adapters: extend coverage before adding more protocol code

| Existing preset | Useful coverage to add | Boundary |
| --- | --- | --- |
| [Jina](https://jina.ai/en-US/embeddings/) | Explicit v5 text small/nano models, task mapping, dimension overrides | Dense text fits the existing result type. Image/audio and late-interaction output need separate types |
| [Voyage](https://docs.voyageai.com/docs/embeddings) | Voyage 4 family and code models; query/document modes | Preserve `output_dimension` and `truncation` translation; keep the configured default stable |
| [Cohere](https://docs.cohere.com/v2/docs/cohere-embed) | Current model families through the native float-vector text path | Rich multimodal inputs and non-float outputs are separate work |
| [Gemini](https://ai.google.dev/gemini-api/docs/embeddings) | Current models through the documented compatible text route, plus explicit dimensions | Native task/title and multimodal options need their own mapping; model replacement requires re-embedding |
| [Mistral](https://docs.mistral.ai/api/endpoint/embeddings) | A dimensions regression test and correct `output_dimension` mapping | Current Sema generic code sends `dimensions` for every non-Voyage provider. Default-width smoke tests do not detect this mismatch |
| OpenAI, Together, Fireworks | Keep contract and opt-in live tests, including custom endpoints | Access/billing failures are separate from serializer correctness; an alternate host can improve access without adding a new model |

## Popularity evidence

Hugging Face API download counts were captured on the survey date. They are a
rolling download proxy, not unique users, production usage, or API-provider
market share. They support prioritizing Qwen and BGE deployment coverage.

| Model | Reported downloads | Reported likes |
| --- | ---: | ---: |
| [BGE-M3](https://huggingface.co/api/models/BAAI/bge-m3) | 31,379,839 | 3,906 |
| [Qwen3 Embedding 0.6B](https://huggingface.co/api/models/Qwen/Qwen3-Embedding-0.6B) | 8,568,001 | 1,303 |
| [Mixedbread embed large v1](https://huggingface.co/api/models/mixedbread-ai/mxbai-embed-large-v1) | 2,081,255 | 839 |
| [Qwen3 Embedding 8B](https://huggingface.co/api/models/Qwen/Qwen3-Embedding-8B) | 2,033,070 | 835 |
| [Qwen3 Embedding 4B](https://huggingface.co/api/models/Qwen/Qwen3-Embedding-4B) | 1,772,109 | 340 |
| [Harrier OSS v1 0.6B](https://huggingface.co/api/models/microsoft/harrier-oss-v1-0.6b) | 265,052 | 324 |
| [Jina v5 text nano](https://huggingface.co/api/models/jinaai/jina-embeddings-v5-text-nano) | 122,102 | 101 |

Closed-provider popularity cannot be ranked fairly from these counts. SDK
integrations and community discussion indicate visibility, but do not supply
comparable deployment counts. Model age also affects downloads and likes.

## Comparable benchmark snapshot

The following values come from the official MTEB backend's
[MTEB(Multilingual, v2) scores](https://mteb-leaderboard-backend.hf.space/v1/benchmarks/MTEB%28Multilingual%2C%20v2%29/scores).
This is a fixed 131-task reference benchmark, currently hidden from the default
benchmark menu. It is not a claim about the latest overall leaderboard. Only
complete 131-task rows are included. Scores are multiplied by 100; higher is
better. These are published results, not evaluations run by Sema.

| Model | Mean task score | Retrieval score | Reported zero-shot tasks |
| --- | ---: | ---: | ---: |
| Microsoft Harrier OSS v1 27B | 74.27 | 78.27 | 78% |
| Qwen3 Embedding 8B | 70.58 | 70.88 | 99% |
| Qwen3 Embedding 4B | 69.45 | 69.60 | 99% |
| Microsoft Harrier OSS v1 0.6B | 69.01 | 70.75 | 78% |
| Gemini embedding 001 | 68.37 | 67.71 | 99% |
| Jina embeddings v5 text nano | 65.52 | 63.26 | Unknown |
| Qwen3 Embedding 0.6B | 64.34 | 64.65 | 99% |

Training overlap differs, so even a shared benchmark does not establish an
equally unseen test set. Harrier is a model to evaluate, not evidence that a
particular hosted API offers it. Qwen's host variants, quantized builds,
instructions, and pooling settings can differ from the published evaluation.

BGE-M3, OpenAI text-embedding-3-large, and Jina v5 text small have incomplete
rows in this snapshot. Voyage 4 and Cohere v4 have too few reported tasks here
for a comparable aggregate; no complete comparable score was established for
newer Cohere models or Gemini embedding 2. Absence of a score is not evidence
of poor quality. Vendor retrieval-only evaluations, English MTEB, multimodal
benchmarks, and agent/search-system scores must not be mixed into this table.

Use a small Sema-specific retrieval evaluation before choosing a default:
English and Norwegian prose, code search, long documents, query/document task
selection, and a fixed set of relevant document IDs. Record Recall@k/nDCG,
latency, cost, dimensions, normalization, model revision, and server settings.
Rebuild the index for each embedding configuration.

The adjacent [data snapshot](2026-10-21-embedding-provider-survey-data.json)
preserves selected raw aggregate values, task coverage, source URLs, and model
adoption counts. It does not duplicate the full leaderboard.

## Refactor proposal: make provider additions small

The current implementation has duplicated provider metadata, a boolean for one
protocol exception, and provider-local execution policies. These increase the
chance that a new provider works in explicit configuration but fails in automatic
selection or with optional request fields.

1. **Create one internal provider registry.** A private
   `EmbeddingProviderSpec` can own the preset name, environment aliases, default
   model, endpoint construction, request dialect, rerank availability, and
   documented batch limits. Use it from
   `crates/sema-llm/src/builtins/embedding_config.rs` and the automatic selection
   table in `crates/sema-llm/src/builtins/chat.rs`. Preserve the current selection
   order explicitly. Keep endpoint/account/region overrides available.
2. **Replace `voyage: bool` with explicit protocol translation.** In
   `crates/sema-llm/src/embeddings.rs`, use a private enum or small encoder/decoder
   interface for the actual differences: OpenAI, Voyage, Mistral, and other
   documented variants. Keep native Nomic, Cohere, and Ollama adapters. Fix the
   Mistral dimension key with a failing wire test first. Validate option support
   at this boundary rather than forwarding arbitrary vendor fields from Sema
   builtins. Do not rewrite the public `LlmProvider` trait to add presets.
   Keep model-specific instructions, dimensions, and limits separate from the
   host's protocol. Serving Qwen and BGE from the same URL does not give them
   identical query/document preprocessing rules.
3. **Share execution policy where behavior is established.** Extract bounded
   batching, ordered aggregation, checked usage totals, cancellation, timeouts,
   and bounded retry for 429/5xx/network failures. Retry only a failed chunk;
   replaying successful chunks can incur additional charges. Honor retry hints
   and existing project limits. Preserve cassette identities and accounting.
   Provider count limits and token limits are different; do not infer tokens
   from character counts. Integrate the strict response validation from
   [PR #173](https://github.com/sema-lisp/sema/pull/173) instead of duplicating it.
4. **Make presets data plus tests.** Each compatible preset should need a
   registry entry, documented model/endpoint choices, and contract cases in the
   existing LLM test suite. Test authentication/URL, field translation, reordered
   response indices, malformed vectors, batch limits, usage, and both dispatch
   paths. Live tests remain opt-in and record the exact model and endpoint.

Steps 1 and 2 are the small prerequisite for the proposed preset additions.
Shared retry and execution policy can follow separately with focused regression
tests; it should not turn a provider-list change into a runtime rewrite. Until
then, retain Nomic's tested batching and document the lack of embedding retries.

Keep `EmbedRequest` text inputs and `EmbedResponse` dense float vectors stable
in this round. Images/audio, sparse vectors, multivectors, binary outputs, and
managed search are distinct capabilities. Add typed alternatives later while
preserving the existing `llm/embed` interface. Avoid adding another public
`EmbedOptions` field for every vendor parameter; public Rust struct additions
can break downstream struct literals.

Record provider, model/revision, dimensions, task mode, and normalization with
stored indexes. Never mix vectors from different embedding spaces merely
because they have the same length. New provider keywords do not add builtin
names, so this scope does not require editor grammar changes or plugin releases.

## Discovery and evidence quality

The survey used Hacker News, Reddit, alternative/comparison searches, and
awesome lists to find candidates. Final protocol claims use provider documents
or source code; final comparable scores use the benchmark's own backend.

- Hacker News discussions included [Jina embeddings](https://news.ycombinator.com/item?id=38020109),
  [Gemini embeddings](https://news.ycombinator.com/item?id=44747457), and
  [Qwen3 Embedding](https://news.ycombinator.com/item?id=44199081).
- A [Reddit deployment discussion](https://www.reddit.com/r/LocalLLaMA/comments/1nezmfi/real_life_experience_with_qwen3_embeddings/)
  raised quantization, pooling, and instruction-handling concerns. These are
  anecdotes that motivate deployment-specific tests, not confirmed defects
  in the underlying model.
- [Awesome Embedding Model API](https://github.com/ishandutta2007/Awesome-Embedding-Model-API)
  supplied candidate names. Its unsourced market/revenue estimates and stale
  prices were not used for ranking.
- Cross-provider comparison searches, including
  [TheRouter's embedding/reranking comparison](https://therouter.ai/blog/llm-api-embeddings-reranking-cross-provider-comparison/),
  were discovery inputs. A comparison page's provider list does not establish
  a current supported endpoint or a comparable benchmark.
- The [OpenRouter catalog](https://openrouter.ai/api/v1/embeddings/models),
  provider API references, official SDKs, the
  [MTEB backend schema](https://mteb-leaderboard-backend.hf.space/openapi.json),
  and Hugging Face model metadata were used for the concrete checks.

Important corrections from checking primary sources: Azure v1 supports bearer
API keys; Alibaba has a dense-text compatible API; Bedrock now supports bearer
API keys; Jina v5 text does not require a multimodal result type; and Mixedbread's
older SDK is not proof that its direct embedding API is a current product.

## Nomic confidence and remaining release work

The cross-language review found a material batch-limit issue before release.
The official Python SDK and its [limit-change PR](https://github.com/nomic-ai/nomic/pull/392)
support at most ten texts per request. Some JavaScript and Java integrations
still use much larger batches, so agreement among third-party wrappers alone
would have been misleading. PR #172 now chunks at ten, sets truncation explicitly,
checks supported v1.5 dimensions, preserves order, and checks usage totals.

The live smoke test subsequently passed on 2026-10-21 with
`nomic-embed-text-v1.5`, confirming account access, the tested dimensions/tasks,
and batching across the ten-text request boundary. Synthetic fixtures still
cover error cases that the live smoke does not exercise. The adapter currently
does not retry embedding requests automatically. See the validation plan for
the SDK/source cross-check, recorded results, and the repeat test required on
the final combined release commit.

## Implementation and validation — 2026-10-21

The provider round implements the registry and protocol refactor above.
`embedding_presets.rs` owns names, environment aliases, defaults, endpoint
construction, protocol selection, and rerank availability. Explicit configuration
and automatic discovery use that registry. `EmbeddingDialect` owns request
translation and the documented model batch limits. Native Nomic, Cohere, and
Ollama adapters remain in place. No public Rust request fields or trait methods
were added. New compatible providers generally require a registry entry, contract
tests, and setup documentation; a distinct wire protocol also needs a dialect.

Alibaba v3/v4 requests split at ten texts, qwen3.7 at twenty, and TEI at 32.
The common compatible executor preserves input order, checks model/vector width
across chunks, checks token-total overflow, and returns no partial result on
failure. It uses the strict response parser from PR #173. Automatic retry remains
deferred; timeout applies to each request. Custom compatible endpoints retain
their extension fields. Known protocols reject unsupported options before HTTP.

Validation includes 163 passing LLM integration tests and 133 passing LLM library
tests. The fixtures cover URL/auth construction, option translation, environment
selection, chat isolation, sync/spawned calls, reordered indices, batching,
malformed responses, and failure after an earlier successful chunk. Initial
regressions reproduced the missing provider defaults/discovery, wrong Mistral
dimension field, and unbounded Alibaba batch. Review also caught and fixed an
automatic-selection regression that admitted a selected legacy provider without
credentials. Formatting, focused Clippy checks, builtin-doc checks, runtime
source policy, and rebuilt embedded-runtime freshness passed. Clippy locally
allows the existing Rust 1.99 `fetch_update` deprecation in unrelated crates.

| Provider | Live model/setup | Result |
| --- | --- | --- |
| OpenRouter | `openai/text-embedding-3-small` | Passed sync batch, Unicode/duplicate similarity, spawned single, and 64 dimensions |
| Azure OpenAI v1 | `text-embedding-3-small` version 1, deployment name supplied as model | Passed the same checks, including bearer resource-key auth and 64 dimensions |
| Cloudflare Workers AI | `@cf/baai/bge-m3` | Passed sync batch, Unicode/duplicate similarity, and spawned single |
| TEI | Native Metal server 1.9.4, `BAAI/bge-small-en-v1.5` | Passed common checks and 35 texts across the 32-item boundary |
| Nomic (regression) | `nomic-embed-text-v1.5` | Existing live release gate passed after the refactor |
| DeepInfra | `Qwen/Qwen3-Embedding-0.6B` | Passed sync batch, Unicode/duplicate similarity, and spawned single after account funding |
| Alibaba | `text-embedding-v4` | Mock contract tests passed; account verification pending before live key/endpoint setup |

To repeat an individual new-provider gate, configure its documented environment
variables and run (replace `azure` with the preset name):

```bash
cargo test -p sema-lang --test llm_suite \
  embedding_config_test::live_azure_embedding_release_gate -- --ignored --exact
```

The six ignored live tests fail when configuration is missing; they do not report
a skipped request as success. The common smoke program also runs against a mock
server in ordinary CI. Alibaba still needs live verification before the provider
round is released. Repeat the live gates and the full `jake ci`
release suite on the final combined commit. No release was cut by this work.
