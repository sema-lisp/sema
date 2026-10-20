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
    /// Nomic: `top_n` parameter, `results` array. Default model `nomic-rerank-v1.5`.
    Nomic,
    /// Together AI: `top_n` parameter, `results` array. Default model `BAAI/bge-reranker-v2-m3`.
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
            RerankDialect::Together => "BAAI/bge-reranker-v2-m3",
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
    /// Set for providers that also offer a hosted reranker (Jina, Voyage).
    rerank: Option<RerankDialect>,
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
            base_url,
            default_model,
            rerank: None,
            client: crate::http::create_client(None)?,
        })
    }

    /// Enable hosted reranking via the given wire dialect (Jina / Voyage).
    pub fn with_rerank(mut self, dialect: RerankDialect) -> Self {
        self.rerank = Some(dialect);
        self
    }

    async fn rerank_async(
        &self,
        request: RerankRequest,
        dialect: RerankDialect,
    ) -> Result<RerankResponse, LlmError> {
        let model = request
            .model
            .unwrap_or_else(|| dialect.default_model().to_string());
        let document_count = request.documents.len();
        let top_k = request.top_k;
        let mut body = serde_json::json!({
            "model": model,
            "query": request.query,
            "documents": request.documents,
        });
        if let Some(k) = request.top_k {
            body[dialect.top_k_param()] = serde_json::json!(k);
        }
        let resp = self
            .client
            .post(format!("{}/rerank", self.base_url))
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

    async fn embed_async(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| self.default_model.clone());
        openai_compat_embed(
            &self.client,
            &self.base_url,
            &self.api_key,
            model,
            request.texts,
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
) -> Result<EmbedResponse, LlmError> {
    let input_count = texts.len();
    let body = serde_json::json!({
        "input": texts,
        "model": model,
    });

    let resp = client
        .post(format!("{base_url}/embeddings"))
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

    let embeddings = parse_embedding_data(&api_resp)?;

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

    EmbedResponse {
        embeddings,
        model: resp_model,
        usage,
    }
    .validated(input_count)
}

impl LlmProvider for OpenAiCompatEmbeddingProvider {
    fn name(&self) -> &str {
        &self.name
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
    default_model: String,
    client: reqwest::Client,
}

impl CohereEmbeddingProvider {
    pub fn new(api_key: String, default_model: Option<String>) -> Result<Self, LlmError> {
        Ok(Self {
            api_key,
            default_model: default_model.unwrap_or_else(|| "embed-english-v3.0".to_string()),
            client: crate::http::create_client(None)?,
        })
    }

    async fn embed_async(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let input_count = request.texts.len();
        let model = request.model.unwrap_or_else(|| self.default_model.clone());
        let body = serde_json::json!({
            "model": model,
            "texts": request.texts,
            "input_type": "search_document",
            "embedding_types": ["float"],
        });

        let resp = self
            .client
            .post("https://api.cohere.com/v2/embed")
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
        let model = request.model.unwrap_or_else(|| "rerank-v3.5".to_string());
        let document_count = request.documents.len();
        let top_k = request.top_k;
        let mut body = serde_json::json!({
            "model": model,
            "query": request.query,
            "documents": request.documents,
        });
        if let Some(k) = request.top_k {
            body["top_n"] = serde_json::json!(k);
        }
        let resp = self
            .client
            .post("https://api.cohere.com/v2/rerank")
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
    fn name(&self) -> &str {
        "cohere"
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
/// every input exactly once. Mixed or invalid indices must never reorder silently.
fn parse_embedding_data(response: &serde_json::Value) -> Result<Vec<Vec<f64>>, LlmError> {
    let data = response
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| LlmError::Parse("missing data in embedding response".into()))?;
    let indexed = data.iter().any(|row| row.get("index").is_some());
    let mut vectors = vec![None; data.len()];
    for (position, row) in data.iter().enumerate() {
        let index = if indexed {
            row.get("index")
                .and_then(|v| v.as_u64())
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| LlmError::Parse("missing or invalid embedding index".into()))?
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
    fn compatible_hosts_can_omit_all_indices_but_cannot_mix_them() {
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
