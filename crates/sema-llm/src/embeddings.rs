use crate::provider::LlmProvider;
use crate::types::{
    ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, LlmError, RerankRequest,
    RerankResponse, RerankResult, Usage,
};

/// The hosted-rerank wire dialects behind the OpenAI-compatible embedding providers.
/// They share `POST {base}/rerank` + `{model, query, documents}` but differ in the
/// top-K parameter name, the results array key, or the default model.
#[derive(Clone, Copy)]
pub enum RerankDialect {
    /// Jina: `top_n` parameter, `results` array. Default model `jina-reranker-v2-base-multilingual`.
    Jina,
    /// Voyage: `top_k` parameter, `data` array. Default model `rerank-2.5`.
    Voyage,
    /// Together AI: `top_n` parameter, `results` array. Default model `mixedbread-ai/mxbai-rerank-large-v2`.
    Together,
    /// Fireworks AI: `top_n` parameter, `data` array. Default model `fireworks/qwen3-reranker-8b`.
    Fireworks,
}

impl RerankDialect {
    fn default_model(self) -> &'static str {
        match self {
            RerankDialect::Jina => "jina-reranker-v2-base-multilingual",
            RerankDialect::Voyage => "rerank-2.5",
            RerankDialect::Together => "mixedbread-ai/mxbai-rerank-large-v2",
            RerankDialect::Fireworks => "fireworks/qwen3-reranker-8b",
        }
    }
    fn top_k_param(self) -> &'static str {
        match self {
            RerankDialect::Jina => "top_n",
            RerankDialect::Voyage => "top_k",
            RerankDialect::Together => "top_n",
            RerankDialect::Fireworks => "top_n",
        }
    }
    fn results_key(self) -> &'static str {
        match self {
            RerankDialect::Jina => "results",
            RerankDialect::Voyage => "data",
            RerankDialect::Together => "results",
            RerankDialect::Fireworks => "data",
        }
    }
}

/// Parse the shared `[{index, relevance_score}]` rerank result array under `key`.
fn parse_rerank_results(
    api_resp: &serde_json::Value,
    key: &str,
) -> Result<Vec<RerankResult>, LlmError> {
    let arr = api_resp
        .get(key)
        .and_then(|r| r.as_array())
        .ok_or_else(|| LlmError::Parse(format!("missing {key} in rerank response")))?;
    Ok(arr
        .iter()
        .filter_map(|item| {
            let index = item.get("index").and_then(|v| v.as_u64())? as usize;
            let score = item.get("relevance_score").and_then(|v| v.as_f64())?;
            Some(RerankResult { index, score })
        })
        .collect())
}

/// An embedding-only provider that uses OpenAI-compatible embed API.
/// Works for Jina, Voyage, and other providers with the same format.
pub struct OpenAiCompatEmbeddingProvider {
    name: String,
    api_key: String,
    base_url: String,
    default_model: String,
    embedding_options: crate::types::EmbedOptions,
    /// Set for providers that also offer a hosted reranker (Jina, Voyage).
    rerank: Option<RerankDialect>,
    rerank_model: Option<String>,
    rerank_top_k: Option<usize>,
    client: reqwest::Client,
}

impl OpenAiCompatEmbeddingProvider {
    pub fn new(
        name: String,
        api_key: String,
        base_url: String,
        default_model: String,
    ) -> Result<Self, LlmError> {
        Ok(Self {
            name,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            default_model,
            embedding_options: Default::default(),
            rerank: None,
            rerank_model: None,
            rerank_top_k: None,
            client: crate::http::create_client(None)?,
        })
    }

    /// Enable hosted reranking via the given wire dialect (Jina / Voyage).
    pub fn with_rerank(mut self, dialect: RerankDialect) -> Self {
        self.rerank = Some(dialect);
        self
    }

    /// Set a default result limit for reranking.
    pub fn with_rerank_top_k(mut self, top_k: Option<usize>) -> Self {
        self.rerank_top_k = top_k;
        self
    }

    /// Override the reranker model independently of the embedding model.
    pub fn with_rerank_model(mut self, model: Option<String>) -> Self {
        self.rerank_model = model;
        self
    }

    async fn rerank_async(
        &self,
        request: RerankRequest,
        dialect: RerankDialect,
    ) -> Result<RerankResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| {
            self.rerank_model
                .clone()
                .unwrap_or_else(|| dialect.default_model().to_string())
        });
        let mut body = serde_json::json!({
            "model": model,
            "query": request.query,
            "documents": request.documents,
        });
        if let Some(k) = request.top_k.or(self.rerank_top_k) {
            body[dialect.top_k_param()] = serde_json::json!(k);
        }
        let resp = crate::http::with_timeout(
            self.client.post(format!("{}/rerank", self.base_url)),
            request.timeout_ms.or(self.embedding_options.timeout_ms),
        )
        .header("Authorization", format!("Bearer {}", self.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
        let resp = crate::http::check_status(resp).await?;
        let api_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        let results = parse_rerank_results(&api_resp, dialect.results_key())?;
        Ok(RerankResponse { results, model })
    }

    /// Set defaults for options that individual embedding requests may override.
    pub fn with_embedding_options(mut self, options: crate::types::EmbedOptions) -> Self {
        self.embedding_options = options;
        self
    }

    async fn embed_async(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| self.default_model.clone());
        openai_compat_embed(
            &self.client,
            &self.base_url,
            &self.api_key,
            model,
            request.texts,
            &request.options.with_defaults(&self.embedding_options),
            self.name == "voyage",
        )
        .await
    }
}

/// One `POST {base_url}/embeddings` in the OpenAI wire format, shared by the
/// OpenAI provider and every OpenAI-compatible embedding host (Jina, Voyage,
/// Mistral, ...). `usage.prompt_tokens` falls back to `total_tokens`, which
/// some hosts report instead.
pub(crate) async fn openai_compat_embed(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: String,
    texts: Vec<String>,
    options: &crate::types::EmbedOptions,
    voyage: bool,
) -> Result<EmbedResponse, LlmError> {
    let mut body = serde_json::json!({
        "input": texts,
        "model": model,
    });

    if let Some(n) = options.dimensions {
        body[if voyage {
            "output_dimension"
        } else {
            "dimensions"
        }] = serde_json::json!(n);
    }
    if let Some(s) = &options.input_type {
        body["input_type"] = serde_json::json!(s);
    }
    if let Some(s) = &options.task {
        body["task"] = serde_json::json!(s);
    }
    if let Some(b) = options.truncate {
        body[if voyage { "truncation" } else { "truncate" }] = serde_json::json!(b);
    }
    if let Some(v) = &options.keep_alive {
        body["keep_alive"] = v.clone();
    }

    let resp = crate::http::with_timeout(
        client.post(format!("{}/embeddings", base_url.trim_end_matches('/'))),
        options.timeout_ms,
    )
    .header("Authorization", format!("Bearer {api_key}"))
    .header("Content-Type", "application/json")
    .json(&body)
    .send()
    .await
    .map_err(|e| LlmError::Http(e.to_string()))?;

    let resp = crate::http::check_status(resp).await?;

    let api_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| LlmError::Parse(e.to_string()))?;

    let resp_model = api_resp
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or(&model)
        .to_string();

    let embeddings = api_resp
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| LlmError::Parse("missing data in embedding response".to_string()))?
        .iter()
        .filter_map(|item| {
            item.get("embedding")
                .and_then(|e| e.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>())
        })
        .collect::<Vec<Vec<f64>>>();

    let usage = api_resp
        .get("usage")
        .map(|u| Usage {
            prompt_tokens: u
                .get("prompt_tokens")
                .or(u.get("total_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            completion_tokens: 0,
            model: resp_model.clone(),
            ..Default::default()
        })
        .unwrap_or_default();

    Ok(EmbedResponse {
        embeddings,
        model: resp_model,
        usage,
    })
}

impl LlmProvider for OpenAiCompatEmbeddingProvider {
    fn embedding_endpoint(&self) -> Option<&str> {
        Some(&self.base_url)
    }

    fn default_rerank_top_k(&self) -> Option<usize> {
        self.rerank_top_k
    }

    fn default_rerank_model(&self) -> &str {
        self.rerank_model
            .as_deref()
            .unwrap_or_else(|| self.rerank.map_or("", RerankDialect::default_model))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn embedding_options(&self) -> crate::types::EmbedOptions {
        self.embedding_options.clone()
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Config(format!(
            "{} does not support chat completions (embedding-only provider)",
            self.name
        )))
    }

    fn embed(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        sema_io::io_block_on(self.embed_async(request))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn embed_future(&self, request: EmbedRequest) -> Option<crate::provider::BoxEmbedFuture<'_>> {
        Some(Box::pin(self.embed_async(request)))
    }

    fn rerank(&self, request: RerankRequest) -> Result<RerankResponse, LlmError> {
        match self.rerank {
            Some(dialect) => sema_io::io_block_on(self.rerank_async(request, dialect)),
            None => Err(LlmError::Config(format!(
                "{} does not support reranking",
                self.name
            ))),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn rerank_future(
        &self,
        request: RerankRequest,
    ) -> Option<crate::provider::BoxRerankFuture<'_>> {
        let dialect = self.rerank?;
        Some(Box::pin(self.rerank_async(request, dialect)))
    }
}

/// Cohere embedding provider — unique API format.
pub struct CohereEmbeddingProvider {
    api_key: String,
    base_url: String,
    rerank_model: String,
    rerank_top_k: Option<usize>,
    default_model: String,
    embedding_options: crate::types::EmbedOptions,
    client: reqwest::Client,
}

impl CohereEmbeddingProvider {
    pub fn new(api_key: String, default_model: Option<String>) -> Result<Self, LlmError> {
        Ok(Self {
            api_key,
            base_url: "https://api.cohere.com/v2".to_string(),
            rerank_model: "rerank-v3.5".to_string(),
            rerank_top_k: None,
            embedding_options: Default::default(),
            default_model: default_model.unwrap_or_else(|| "embed-english-v3.0".to_string()),
            client: crate::http::create_client(None)?,
        })
    }

    /// Override the Cohere-compatible API root (without /embed or /rerank).
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    /// Set a default result limit for reranking.
    pub fn with_rerank_top_k(mut self, top_k: Option<usize>) -> Self {
        self.rerank_top_k = top_k;
        self
    }

    /// Override the reranker model independently of the embedding model.
    pub fn with_rerank_model(mut self, model: Option<String>) -> Self {
        if let Some(model) = model {
            self.rerank_model = model;
        }
        self
    }

    /// Set defaults for options that individual embedding requests may override.
    pub fn with_embedding_options(mut self, options: crate::types::EmbedOptions) -> Self {
        self.embedding_options = options;
        self
    }

    async fn embed_async(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| self.default_model.clone());
        let options = request.options.with_defaults(&self.embedding_options);
        let mut body = serde_json::json!({
            "model": model,
            "texts": request.texts,
            "input_type": options.input_type.as_deref().unwrap_or("search_document"),
            "embedding_types": ["float"],
        });

        if let Some(n) = options.dimensions {
            body["output_dimension"] = serde_json::json!(n);
        }
        if let Some(b) = options.truncate {
            body["truncate"] = serde_json::json!(if b { "END" } else { "NONE" });
        }
        let resp = crate::http::with_timeout(
            self.client.post(format!("{}/embed", self.base_url)),
            options.timeout_ms,
        )
        .header("Authorization", format!("Bearer {}", self.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;

        let resp = crate::http::check_status(resp).await?;

        let api_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        // Cohere nests under embeddings.float
        let embeddings = api_resp
            .pointer("/embeddings/float")
            .and_then(|e| e.as_array())
            .ok_or_else(|| {
                LlmError::Parse("missing embeddings.float in Cohere response".to_string())
            })?
            .iter()
            .filter_map(|item| {
                item.as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>())
            })
            .collect::<Vec<Vec<f64>>>();

        Ok(EmbedResponse {
            embeddings,
            model: model.clone(),
            usage: Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                model,
                ..Default::default()
            },
        })
    }

    async fn rerank_async(&self, request: RerankRequest) -> Result<RerankResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| self.rerank_model.clone());
        let mut body = serde_json::json!({
            "model": model,
            "query": request.query,
            "documents": request.documents,
        });
        if let Some(k) = request.top_k.or(self.rerank_top_k) {
            body["top_n"] = serde_json::json!(k);
        }
        let resp = crate::http::with_timeout(
            self.client.post(format!("{}/rerank", self.base_url)),
            request.timeout_ms.or(self.embedding_options.timeout_ms),
        )
        .header("Authorization", format!("Bearer {}", self.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
        let resp = crate::http::check_status(resp).await?;
        let api_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        let results = parse_rerank_results(&api_resp, "results")?;
        Ok(RerankResponse { results, model })
    }
}

impl LlmProvider for CohereEmbeddingProvider {
    fn embedding_endpoint(&self) -> Option<&str> {
        Some(&self.base_url)
    }

    fn default_rerank_top_k(&self) -> Option<usize> {
        self.rerank_top_k
    }

    fn default_rerank_model(&self) -> &str {
        &self.rerank_model
    }
    fn name(&self) -> &str {
        "cohere"
    }

    fn embedding_options(&self) -> crate::types::EmbedOptions {
        self.embedding_options.clone()
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Config(
            "cohere does not support chat completions (embedding-only provider)".to_string(),
        ))
    }

    fn embed(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        sema_io::io_block_on(self.embed_async(request))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn embed_future(&self, request: EmbedRequest) -> Option<crate::provider::BoxEmbedFuture<'_>> {
        Some(Box::pin(self.embed_async(request)))
    }

    fn rerank(&self, request: RerankRequest) -> Result<RerankResponse, LlmError> {
        sema_io::io_block_on(self.rerank_async(request))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn rerank_future(
        &self,
        request: RerankRequest,
    ) -> Option<crate::provider::BoxRerankFuture<'_>> {
        Some(Box::pin(self.rerank_async(request)))
    }
}
