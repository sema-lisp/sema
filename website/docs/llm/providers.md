---
outline: [2, 3]
---

# Provider Management

## Auto-Configuration

Sema auto-detects and configures all available providers from environment variables on startup. Set the API key and any account-specific endpoint or deployment name required by the provider.

### `llm/auto-configure`

Manually trigger auto-configuration (runs automatically on startup unless `--no-llm` is used).

```sema
(llm/auto-configure)
```

## Manual Configuration

### `llm/configure`

Manually configure a known provider with specific options.

```sema
(llm/configure :anthropic {:api-key "sk-..."})

;; Ollama with custom host
(llm/configure :ollama {:host "http://localhost:11434"
                         :default-model "llama3"})
```

### OpenAI-Compatible Providers

Any provider with an OpenAI-compatible API can be registered by passing `:api-key` and `:base-url` with any provider name — no custom code needed, just configuration.

```sema
;; Together AI
(llm/configure :together
  {:api-key (env "TOGETHER_API_KEY")
   :base-url "https://api.together.xyz/v1"
   :default-model "meta-llama/Llama-3-70b-chat-hf"})

;; Azure OpenAI
(llm/configure :azure
  {:api-key (env "AZURE_OPENAI_API_KEY")
   :base-url "https://my-resource.openai.azure.com/openai/v1"
   :default-model "gpt-4"})

;; Local vLLM / LiteLLM / text-generation-inference
(llm/configure :local
  {:api-key "not-needed"
   :base-url "http://localhost:8000/v1"
   :default-model "my-model"})

;; Once configured, use like any other provider
(llm/complete "Hello from Together!" {:model "meta-llama/Llama-3-70b-chat-hf"})
```

This works for any service that implements the OpenAI chat completions API: Together, Fireworks, Perplexity, Azure OpenAI, Anyscale, vLLM, LiteLLM, text-generation-inference, and others.

> **Sandbox note.** Local endpoints like `http://localhost:8000/v1` and Ollama on `localhost:11434` work normally in the REPL, CLI, and notebook. When running **untrusted code under `--sandbox`**, a `:base-url`/`:host` pointing at a loopback or private address (`localhost`, `127.0.0.1`, `10.x`, `169.254.169.254`, …) is rejected to prevent SSRF. Run unsandboxed to use a local endpoint.

## Lisp-Defined Providers

For full control over request/response handling, you can define providers entirely in Sema using `llm/define-provider`. The provider's `:complete` function receives the request as a map and returns either a string or a response map.

### `llm/define-provider`

```sema
(llm/define-provider :name {:complete fn :default-model "..."})
```

**Parameters:**

- `:complete` — **(required)** A function that takes a request map and returns a response
- `:default-model` — Model name used when none is specified (default: `"default"`)

### Request Map

The `:complete` function receives a map with these keys:

| Key               | Type           | Description                        |
| ----------------- | -------------- | ---------------------------------- |
| `:model`          | string         | Model name                         |
| `:messages`       | list of maps   | Each has `:role` and `:content`    |
| `:max-tokens`     | integer or nil | Token limit                        |
| `:temperature`    | float or nil   | Sampling temperature               |
| `:system`         | string or nil  | System prompt                      |
| `:tools`          | list or nil    | Tool schemas (if tools are in use) |
| `:stop-sequences` | list or nil    | Stop sequences for generation      |

### Response Format

The function can return either:

- **A string** — used as the assistant's response content
- **A map** with optional keys:

| Key            | Type   | Default       |
| -------------- | ------ | ------------- |
| `:content`     | string | `""`          |
| `:role`        | string | `"assistant"` |
| `:model`       | string | request model |
| `:stop-reason` | string | `"end_turn"`  |
| `:usage`       | map    | zero tokens   |
| `:tool-calls`  | list   | empty list    |

The `:usage` map can contain `:prompt-tokens` and `:completion-tokens` (both integers).

The `:tool-calls` list contains maps with `:id` (string), `:name` (string), and `:arguments` (map). This enables Lisp-defined providers to work with tool-calling agents.

### Examples

**Echo provider** — returns the user's message back:

```sema
(llm/define-provider :echo
  {:complete (fn (req)
    (string/append "Echo: " (:content (last (:messages req)))))
   :default-model "echo-v1"})

(llm/complete "hello")  ;; => "Echo: hello"
```

**HTTP proxy** — forward to a custom API:

```sema
(llm/define-provider :my-api
  {:complete (fn (req)
    (define resp (json/decode
      (http/post "https://my-api.example.com/chat"
        {:headers {"Authorization" (string/append "Bearer " (env "MY_API_KEY"))
                   "Content-Type" "application/json"}
         :body (json/encode {:model (:model req)
                             :prompt (:content (last (:messages req)))})})))
    {:content (:text resp)
     :usage {:prompt-tokens (:input-tokens resp)
             :completion-tokens (:output-tokens resp)}})
   :default-model "my-model-v2"})
```

**Mock provider for testing** — deterministic responses without API calls:

```sema
(define responses (list "First response" "Second response" "Third response"))
(define call-count (atom 0))

(llm/define-provider :mock
  {:complete (fn (req)
    (let ((i (deref call-count)))
      (swap! call-count (fn (n) (+ n 1)))
      (nth responses (mod i (length responses)))))
   :default-model "mock-v1"})

;; Now all llm/complete calls return deterministic values
(llm/complete "anything")  ;; => "First response"
(llm/complete "anything")  ;; => "Second response"
```

**Routing provider** — dispatch to different backends by model name:

```sema
(llm/configure :anthropic {:api-key (env "ANTHROPIC_API_KEY")})
(llm/configure :openai {:api-key (env "OPENAI_API_KEY")})

(llm/define-provider :router
  {:complete (fn (req)
    (let ((model (:model req)))
      (cond
        ((string/starts-with? model "claude")
         (begin (llm/set-default :anthropic)
                (llm/complete (:content (last (:messages req))) {:model model})))
        ((string/starts-with? model "gpt")
         (begin (llm/set-default :openai)
                (llm/complete (:content (last (:messages req))) {:model model})))
        (else (error (string/append "Unknown model: " model))))))
   :default-model "claude-sonnet-4-6"})
```

### Switching Between Providers

Lisp-defined providers integrate with the standard provider management functions:

```sema
(llm/define-provider :mock
  {:complete (fn (req) "mock response") :default-model "m1"})

(llm/configure :anthropic {:api-key (env "ANTHROPIC_API_KEY")})

(llm/set-default :mock)      ;; use mock
(llm/complete "test")         ;; => "mock response"

(llm/set-default :anthropic)  ;; switch to real API
(llm/complete "test")         ;; => real API response
```

## Runtime Provider Switching

### `llm/list-providers`

List all configured providers.

```sema
(llm/list-providers)   ; => (:anthropic :gemini :openai ...)
(llm/providers)        ; => same (alias)
```

### `llm/current-provider`

Get the currently active provider and model.

```sema
(llm/current-provider)   ; => {:name :anthropic :model "claude-sonnet-4-6"}
(llm/default-provider)   ; => same (alias)
```

### `llm/set-default`

Switch the active provider at runtime.

```sema
(llm/set-default :openai)
```

## Supported Providers

All providers are auto-configured from environment variables. Use `(llm/configure :provider {...})` for manual setup.

### Chat / Inference Providers

| Provider | Type | Chat | Stream | Tools | Vision | Env Var |
|----------|------|:----:|:------:|:-----:|:------:|---------|
| **Anthropic** | Native | ✅ | ✅ | ✅ | ✅ | `ANTHROPIC_API_KEY` |
| **OpenAI** | Native | ✅ | ✅ | ✅ | ✅ | `OPENAI_API_KEY` |
| **Google Gemini** | Native | ✅ | ✅ | ✅ | ✅ | `GOOGLE_API_KEY` |
| **Ollama** | Native (local) | ✅ | ✅ | ✅ | ✅ ² | `OLLAMA_HOST` |
| **Groq** | OpenAI-compat | ✅ | ✅ | ✅ | — | `GROQ_API_KEY` |
| **xAI** | OpenAI-compat | ✅ | ✅ | ✅ | — | `XAI_API_KEY` |
| **Mistral** | OpenAI-compat | ✅ | ✅ | ✅ | — | `MISTRAL_API_KEY` |
| **Moonshot** | OpenAI-compat | ✅ | ✅ | ✅ | — | `MOONSHOT_API_KEY` |
| **DeepSeek** | OpenAI-compat | ✅ | ✅ | ✅ | — | `DEEPSEEK_API_KEY` |
| **OpenRouter** | OpenAI-compat (meta) | ✅ | ✅ | ✅ | — | `OPENROUTER_API_KEY` |
| **Together AI** | OpenAI-compat | ✅ | ✅ | ✅ | — | `TOGETHER_API_KEY` |
| **Fireworks AI** | OpenAI-compat | ✅ | ✅ | ✅ | — | `FIREWORKS_API_KEY` |
| **Cerebras** | OpenAI-compat | ✅ | ✅ | ✅ | — | `CEREBRAS_API_KEY` |
| **SambaNova** | OpenAI-compat | ✅ | ✅ | ✅ | — | `SAMBANOVA_API_KEY` |
| **Perplexity** | OpenAI-compat | ✅ | ✅ | ✅ | — | `PERPLEXITY_API_KEY` |
| _Any OpenAI-compat_ | `llm/configure` | ✅ | ✅ | ✅ | ✅ | — |
| _Custom Lisp_ | `llm/define-provider` | ✅ | ¹ | ✅ | — | — |

### Embedding / Reranking Providers

| Provider | Embeddings | Reranking | Env Var |
|----------|:----------:|:---------:|---------|
| **OpenAI** | ✅ | — | `OPENAI_API_KEY` |
| **Ollama** | ✅ | — | `OLLAMA_HOST` (optional) |
| **Mistral** | ✅ | — | `MISTRAL_API_KEY` |
| **Gemini** | ✅ | — | `GEMINI_API_KEY` / `GOOGLE_API_KEY` |
| **Jina** | ✅ | ✅ | `JINA_API_KEY` |
| **Voyage** | ✅ | ✅ | `VOYAGE_API_KEY` |
| **Cohere** | ✅ | ✅ | `COHERE_API_KEY` |
| **Nomic** | ✅ | — | `NOMIC_API_KEY` |
| **Together AI** | ✅ | ✅ | `TOGETHER_API_KEY` |
| **Fireworks AI** | ✅ | ✅ | `FIREWORKS_API_KEY` |
| **OpenRouter** | ✅ | — | `OPENROUTER_API_KEY` |
| **DeepInfra** | ✅ | — | `DEEPINFRA_API_KEY` |
| **Azure OpenAI v1** | ✅ | — | `AZURE_OPENAI_API_KEY`, endpoint and deployment |
| **Cloudflare Workers AI** | ✅ | — | `CLOUDFLARE_API_TOKEN`, account ID or endpoint |
| **Alibaba Model Studio** | ✅ | — | `ALIBABA_API_KEY` / `DASHSCOPE_API_KEY`, regional/workspace endpoint |
| **TEI** | ✅ | — | `TEI_MODEL`, optional `TEI_API_KEY` |

¹ Streaming falls back to non-streaming (sends complete response as a single chunk).

² Vision requires a vision-capable model (e.g., `gemma3:4b`, `llava`).

### Default Models

When you don't pass `:default-model` to `llm/configure` (or pin `:model` on a call), each provider uses the following default.

#### Chat providers

| Provider | Default model |
|----------|---------------|
| `:anthropic` | `claude-sonnet-4-6` |
| `:openai` | `gpt-5.5` |
| `:gemini` | `gemini-3.5-flash` |
| `:ollama` | `gemma4` |
| `:groq` | `llama-3.3-70b-versatile` |
| `:xai` | `grok-4.3` |
| `:mistral` | `mistral-large-latest` |
| `:moonshot` | `kimi-k2.6` |
| `:deepseek` | `deepseek-v4-flash` |
| `:openrouter` | `openai/gpt-5.2` |
| `:together` | `meta-llama/Llama-4-Scout-17B-16E-Instruct` |
| `:fireworks` | `accounts/fireworks/models/gpt-oss-120b` |
| `:cerebras` | `gpt-oss-120b` |
| `:sambanova` | `Meta-Llama-3.3-70B-Instruct` |
| `:perplexity` | `sonar-pro` |

#### Embedding providers

| Provider | Default model |
|----------|---------------|
| `:openai` | `text-embedding-3-small` |
| `:ollama` | `embeddinggemma` |
| `:mistral` | `mistral-embed` |
| `:gemini` | `gemini-embedding-001` |
| `:jina` | `jina-embeddings-v3` |
| `:voyage` | `voyage-3` |
| `:cohere` | `embed-english-v3.0` |
| `:nomic` | `nomic-embed-text-v1.5` |
| `:together` | `BAAI/bge-base-en-v1.5` |
| `:fireworks` | `fireworks/qwen3-embedding-8b` |

#### Reranking providers

| Provider | Default model |
|----------|---------------|
| `:jina` | `jina-reranker-v2-base-multilingual` |
| `:voyage` | `rerank-2.5` |
| `:cohere` | `rerank-v3.5` |
| `:together` | `mixedbread-ai/mxbai-rerank-large-v2` |
| `:fireworks` | `fireworks/qwen3-reranker-8b` |

Chat uses `:default-model` / `SEMA_CHAT_MODEL`; embeddings use `:default-model` or
`:model` / `SEMA_EMBEDDING_MODEL`; reranking uses `:rerank-model` / `SEMA_RERANK_MODEL`.
All three accept per-call `:model` overrides. Retrieval configuration is independent
of chat. See [embedding configuration](./embeddings.md#configuration) for endpoint
and request options.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key (also used for embeddings fallback) |
| `GOOGLE_API_KEY` | Google Gemini API key |
| `GROQ_API_KEY` | Groq API key |
| `XAI_API_KEY` | xAI/Grok API key |
| `MISTRAL_API_KEY` | Mistral API key |
| `MOONSHOT_API_KEY` | Moonshot/Kimi API key |
| `DEEPSEEK_API_KEY` | DeepSeek API key |
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `TOGETHER_API_KEY` | Together AI API key (chat + embeddings + reranking) |
| `FIREWORKS_API_KEY` | Fireworks AI API key (chat + embeddings + reranking) |
| `CEREBRAS_API_KEY` | Cerebras API key |
| `SAMBANOVA_API_KEY` | SambaNova API key |
| `PERPLEXITY_API_KEY` | Perplexity API key |
| `OLLAMA_HOST` | Ollama server URL (default: `http://localhost:11434`) |
| `JINA_API_KEY` | Jina embeddings + reranking API key |
| `VOYAGE_API_KEY` | Voyage embeddings + reranking API key |
| `COHERE_API_KEY` | Cohere embeddings + reranking API key |
| `NOMIC_API_KEY` | Nomic hosted text embeddings API key |
| `NOMIC_BASE_URL` | Nomic API root override (default `https://api-atlas.nomic.ai/v1`) |
| `SEMA_CHAT_MODEL` | Default chat model name |
| `SEMA_CHAT_PROVIDER` | Preferred chat provider |
| `SEMA_EMBEDDING_MODEL` | Default embedding model name |
| `SEMA_EMBEDDING_PROVIDER` | Preferred embedding provider |
| `SEMA_RERANK_MODEL` | Default reranking model name |
| `SEMA_RERANK_PROVIDER` | Preferred reranking provider |

