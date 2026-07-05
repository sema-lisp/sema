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
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }
        let api_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        let results = parse_rerank_results(&api_resp, dialect.results_key())?;
        Ok(RerankResponse { results, model })
    }

    async fn embed_async(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| self.default_model.clone());
        let body = serde_json::json!({
            "input": request.texts,
            "model": model,
        });

        let resp = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

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

        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

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
        let model = request.model.unwrap_or_else(|| "rerank-v3.5".to_string());
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
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }
        let api_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        let results = parse_rerank_results(&api_resp, "results")?;
        Ok(RerankResponse { results, model })
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
}
