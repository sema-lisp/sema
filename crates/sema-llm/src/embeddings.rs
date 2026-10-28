use crate::provider::LlmProvider;
use crate::types::{
    ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, LlmError, RerankRequest,
    RerankResponse, RerankResult, Usage,
};

/// Request differences belong to the provider adapter, not Sema builtins.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddingDialect {
    /// Preserve extension fields for custom OpenAI-compatible endpoints.
    Compatible,
    OpenAi,
    Jina,
    Voyage,
    Mistral,
    Cloudflare,
    Alibaba,
    Tei,
}

impl EmbeddingDialect {
    fn batch_limit(self, model: &str) -> Option<usize> {
        match (self, model) {
            (Self::Alibaba, "text-embedding-v3" | "text-embedding-v4") => Some(10),
            (Self::Alibaba, "qwen3.7-text-embedding") => Some(20),
            // TEI's default --max-client-batch-size; the setup recipe pins it.
            (Self::Tei, _) => Some(32),
            _ => None,
        }
    }

    fn request_body(
        self,
        texts: &[String],
        model: &str,
        options: &crate::types::EmbedOptions,
    ) -> Result<serde_json::Value, LlmError> {
        let unsupported = [
            (
                "dimensions",
                options.dimensions.is_some(),
                !matches!(self, Self::Cloudflare | Self::Tei),
            ),
            (
                "input-type",
                options.input_type.is_some(),
                matches!(self, Self::Compatible | Self::Voyage),
            ),
            (
                "task",
                options.task.is_some(),
                matches!(self, Self::Compatible | Self::Jina),
            ),
            (
                "truncate",
                options.truncate.is_some(),
                matches!(self, Self::Compatible | Self::Voyage | Self::Jina),
            ),
            (
                "keep-alive",
                options.keep_alive.is_some(),
                self == Self::Compatible,
            ),
            ("long-text-mode", options.long_text_mode.is_some(), false),
            (
                "max-tokens-per-text",
                options.max_tokens_per_text.is_some(),
                false,
            ),
        ];
        for (name, supplied, supported) in unsupported {
            if supplied && !supported {
                return Err(LlmError::Config(format!(
                    "this embedding protocol does not support :{name}"
                )));
            }
        }
        if options.dimensions == Some(0) || options.timeout_ms == Some(0) {
            return Err(LlmError::Config(
                "embedding dimensions and timeout must be positive".into(),
            ));
        }
        let mut body = serde_json::json!({"input": texts, "model": model});
        if matches!(
            self,
            Self::OpenAi | Self::Mistral | Self::Alibaba | Self::Tei
        ) {
            body["encoding_format"] = serde_json::json!("float");
        }
        if let Some(n) = options.dimensions {
            let field = if matches!(self, Self::Voyage | Self::Mistral) {
                "output_dimension"
            } else {
                "dimensions"
            };
            body[field] = serde_json::json!(n);
        }
        if let Some(s) = &options.input_type {
            body["input_type"] = serde_json::json!(s);
        }
        if let Some(s) = &options.task {
            body["task"] = serde_json::json!(s);
        }
        if let Some(b) = options.truncate {
            body[if self == Self::Voyage {
                "truncation"
            } else {
                "truncate"
            }] = serde_json::json!(b);
        }
        if let Some(v) = &options.keep_alive {
            body["keep_alive"] = v.clone();
        }
        Ok(body)
    }
}

/// The hosted-rerank wire dialects behind the OpenAI-compatible embedding providers.
/// They share `POST {base}/rerank` + `{model, query, documents}` but differ in the
/// top-K parameter name, the results array key, or the default model.
#[derive(Clone, Copy)]
pub enum RerankDialect {
    /// Jina: `top_n` parameter, `results` array. Default model `jina-reranker-v2-base-multilingual`.
    Jina,
    /// Voyage: `top_k` parameter, `data` array. Default model `rerank-2.5`.
    Voyage,
    /// Legacy custom-host dialect. Nomic's hosted embedding adapter does not use it.
    Nomic,
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
            RerankDialect::Nomic => "nomic-rerank-v1.5",
            RerankDialect::Together => "mixedbread-ai/mxbai-rerank-large-v2",
            RerankDialect::Fireworks => "fireworks/qwen3-reranker-8b",
        }
    }
    fn top_k_param(self) -> &'static str {
        match self {
            RerankDialect::Jina => "top_n",
            RerankDialect::Voyage => "top_k",
            RerankDialect::Nomic => "top_n",
            RerankDialect::Together => "top_n",
            RerankDialect::Fireworks => "top_n",
        }
    }
    fn results_key(self) -> &'static str {
        match self {
            RerankDialect::Jina => "results",
            RerankDialect::Voyage => "data",
            RerankDialect::Nomic => "results",
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
    #[derive(serde::Deserialize)]
    struct Row {
        index: usize,
        relevance_score: f64,
    }
    arr.iter()
        .map(|item| {
            let row: Row = serde_json::from_value(item.clone())
                .map_err(|e| LlmError::Parse(format!("invalid rerank result: {e}")))?;
            Ok(RerankResult {
                index: row.index,
                score: row.relevance_score,
            })
        })
        .collect()
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
    dialect: EmbeddingDialect,
}

impl OpenAiCompatEmbeddingProvider {
    pub fn new(
        name: String,
        api_key: String,
        base_url: String,
        default_model: String,
    ) -> Result<Self, LlmError> {
        Ok(Self {
            dialect: crate::embedding_presets::find(&name)
                .map_or(EmbeddingDialect::Compatible, |preset| preset.dialect),
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
        let document_count = request.documents.len();
        let top_k = request.top_k.or(self.rerank_top_k);
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
        RerankResponse { results, model }.validated(document_count, top_k)
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
            self.dialect,
        )
        .await
    }
}

/// Execute compatible embedding requests with documented model-specific limits.
/// Chunks are sequential; cancellation drops the current and remaining work.
pub(crate) async fn openai_compat_embed(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: String,
    texts: Vec<String>,
    options: &crate::types::EmbedOptions,
    dialect: EmbeddingDialect,
) -> Result<EmbedResponse, LlmError> {
    // Validate even empty requests without making an HTTP call.
    dialect.request_body(&[], &model, options)?;
    let mut combined = EmbedResponse {
        embeddings: Vec::with_capacity(texts.len()),
        model: model.clone(),
        usage: Usage {
            model: model.clone(),
            ..Default::default()
        },
    };
    let mut width = None;
    let limit = dialect.batch_limit(&model).unwrap_or(texts.len().max(1));
    for chunk in texts.chunks(limit) {
        let body = dialect.request_body(chunk, &model, options)?;
        let response = openai_compat_embed_chunk(
            client,
            base_url,
            api_key,
            &model,
            body,
            chunk.len(),
            options,
        )
        .await?;
        let chunk_width = response.embeddings.first().map(Vec::len);
        if let Some(expected) = width {
            if chunk_width != Some(expected) || response.model != combined.model {
                return Err(LlmError::Parse(
                    "embedding model or dimensions changed between batches".into(),
                ));
            }
        } else {
            width = chunk_width;
            combined.model = response.model.clone();
            combined.usage.model = response.model;
        }
        combined.usage.prompt_tokens = combined
            .usage
            .prompt_tokens
            .checked_add(response.usage.prompt_tokens)
            .ok_or_else(|| LlmError::Parse("embedding input usage exceeds u32".into()))?;
        combined.embeddings.extend(response.embeddings);
    }
    Ok(combined)
}

async fn openai_compat_embed_chunk(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    body: serde_json::Value,
    input_count: usize,
    options: &crate::types::EmbedOptions,
) -> Result<EmbedResponse, LlmError> {
    let mut call = crate::http::with_timeout(
        client.post(format!("{}/embeddings", base_url.trim_end_matches('/'))),
        options.timeout_ms,
    );
    if !api_key.is_empty() {
        call = call.bearer_auth(api_key);
    }
    let resp = call
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
        .unwrap_or(model)
        .to_string();

    let embeddings = parse_embedding_data(&api_resp)?;

    let prompt_tokens = api_resp
        .get("usage")
        .and_then(|usage| {
            usage
                .get("prompt_tokens")
                .or_else(|| usage.get("total_tokens"))
        })
        .map(|value| {
            value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| LlmError::Parse("invalid embedding input usage".into()))
        })
        .transpose()?
        .unwrap_or(0);
    let usage = Usage {
        prompt_tokens,
        model: resp_model.clone(),
        ..Default::default()
    };

    EmbedResponse {
        embeddings,
        model: resp_model,
        usage,
    }
    .validated(input_count)
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
        let input_count = request.texts.len();
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

        parse_cohere_response(&api_resp, model, input_count)
    }

    async fn rerank_async(&self, request: RerankRequest) -> Result<RerankResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| self.rerank_model.clone());
        let document_count = request.documents.len();
        let top_k = request.top_k.or(self.rerank_top_k);
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
        RerankResponse { results, model }.validated(document_count, top_k)
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

/// Compatible hosts may omit indices entirely; indexed responses must identify
/// every input exactly once. Gemini omits the zero index; assigning only that
/// default still rejects ambiguous omissions through duplicate/range checks.
fn parse_embedding_data(response: &serde_json::Value) -> Result<Vec<Vec<f64>>, LlmError> {
    let data = response
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| LlmError::Parse("missing data in embedding response".into()))?;
    let indexed = data.iter().any(|row| row.get("index").is_some());
    let mut vectors = vec![None; data.len()];
    for (position, row) in data.iter().enumerate() {
        let index = if indexed {
            match row.get("index") {
                None => 0,
                Some(value) => value
                    .as_u64()
                    .and_then(|v| usize::try_from(v).ok())
                    .ok_or_else(|| LlmError::Parse("invalid embedding index".into()))?,
            }
        } else {
            position
        };
        let slot = vectors
            .get_mut(index)
            .ok_or_else(|| LlmError::Parse("embedding index out of range".into()))?;
        if slot.is_some() {
            return Err(LlmError::Parse("duplicate embedding index".into()));
        }
        let vector: Vec<f64> = serde_json::from_value(
            row.get("embedding")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| LlmError::Parse(format!("invalid embedding vector: {e}")))?;
        *slot = Some(vector);
    }
    vectors
        .into_iter()
        .map(|v| v.ok_or_else(|| LlmError::Parse("missing embedding index".into())))
        .collect()
}

fn parse_cohere_response(
    response: &serde_json::Value,
    model: String,
    input_count: usize,
) -> Result<EmbedResponse, LlmError> {
    let embeddings =
        serde_json::from_value(response.pointer("/embeddings/float").cloned().ok_or_else(
            || LlmError::Parse("missing embeddings.float in Cohere response".into()),
        )?)
        .map_err(|e| LlmError::Parse(format!("invalid Cohere embeddings: {e}")))?;
    let tokens = response
        .pointer("/meta/billed_units/input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32;
    EmbedResponse {
        embeddings,
        model: model.clone(),
        usage: Usage {
            prompt_tokens: tokens,
            model,
            ..Default::default()
        },
    }
    .validated(input_count)
}

#[cfg(test)]
mod response_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cohere_response_keeps_billed_input_tokens() {
        let response = parse_cohere_response(&json!({"embeddings":{"float":[[0.5,1.0]]}, "meta":{"billed_units":{"input_tokens":17}}}), "embed-model".into(), 1).unwrap();
        assert_eq!(response.usage.prompt_tokens, 17);
        assert_eq!(response.usage.model, "embed-model");
    }

    #[test]
    fn cohere_rejects_invalid_vectors_and_counts() {
        for vectors in [
            json!([[1.0, "bad"]]),
            json!([null]),
            json!([[]]),
            json!([]),
            json!([[1.0], [1.0, 2.0]]),
        ] {
            assert!(
                parse_cohere_response(&json!({"embeddings":{"float":vectors}}), "model".into(), 1)
                    .is_err(),
                "{vectors}"
            );
        }
    }

    #[test]
    fn gemini_may_omit_the_zero_index_in_an_indexed_batch() {
        // Captured shape from the live Gemini OpenAI-compatible endpoint.
        for data in [
            json!([{"embedding":[1.0]},{"index":1,"embedding":[2.0]}]),
            json!([{"index":1,"embedding":[2.0]},{"embedding":[1.0]}]),
        ] {
            assert_eq!(
                parse_embedding_data(&json!({"data":data})).unwrap(),
                vec![vec![1.0], vec![2.0]]
            );
        }
        assert!(parse_embedding_data(
            &json!({"data":[{"embedding":[1.0]},{"index":2,"embedding":[2.0]},{"embedding":[3.0]}]})
        )
        .is_err());
    }

    #[test]
    fn compatible_hosts_can_omit_indices_but_not_ambiguously() {
        assert_eq!(
            parse_embedding_data(&json!({"data":[{"embedding":[1.0]},{"embedding":[2.0]}]}))
                .unwrap(),
            vec![vec![1.0], vec![2.0]]
        );
        for data in [
            json!([{"index":0,"embedding":[1.0]},{"embedding":[2.0]}]),
            json!([{"index":0,"embedding":[1.0]},{"index":0,"embedding":[2.0]}]),
            json!([{"index":null,"embedding":[1.0]}]),
        ] {
            assert!(parse_embedding_data(&json!({"data":data})).is_err());
        }
    }
}
