//! Native Nomic text embeddings: https://docs.nomic.ai/reference/api/embed-text-v-1-embedding-text-post
use serde::Deserialize;
use serde_json::{json, Value};

use crate::provider::LlmProvider;
use crate::types::{
    ChatRequest, ChatResponse, EmbedOptions, EmbedRequest, EmbedResponse, LlmError, Usage,
};

/// Nomic's hosted text embedding API. The base URL includes `/v1`.
pub struct NomicEmbeddingProvider {
    api_key: String,
    base_url: String,
    model: String,
    options: EmbedOptions,
    client: reqwest::Client,
}

impl NomicEmbeddingProvider {
    /// Configure native embeddings independently of chat and reranking.
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        options: EmbedOptions,
    ) -> Result<Self, LlmError> {
        if api_key.trim().is_empty() {
            return Err(LlmError::Config(
                "Nomic requires :api-key or NOMIC_API_KEY".into(),
            ));
        }
        if model.trim().is_empty() {
            return Err(LlmError::Config(
                "Nomic embedding model must not be empty".into(),
            ));
        }
        validate_options(&options)?;
        Ok(Self {
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            options,
            client: crate::http::create_client(None)?,
        })
    }

    async fn embed_async(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let options = request.options.with_defaults(&self.options);
        let model = request.model.as_deref().unwrap_or(&self.model);
        let body = request_body(&request.texts, model, &options)?;
        if request.texts.is_empty() {
            return Ok(EmbedResponse {
                embeddings: Vec::new(),
                model: model.to_string(),
                usage: Usage {
                    model: model.to_string(),
                    ..Default::default()
                },
            });
        }
        let mut call = self
            .client
            .post(format!("{}/embedding/text", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body);
        if let Some(ms) = options.timeout_ms {
            call = call.timeout(std::time::Duration::from_millis(ms));
        }
        let response = call
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;
        let response = crate::http::check_status(response).await?;
        let body = response
            .text()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;
        parse_response(&body, request.texts.len(), model, options.dimensions)
    }
}

fn validate_options(options: &EmbedOptions) -> Result<(), LlmError> {
    if let Some(task) = &options.task {
        if !matches!(
            task.as_str(),
            "search_document" | "search_query" | "classification" | "clustering"
        ) {
            return Err(LlmError::Config(
                "Nomic :task must be search_document, search_query, classification, or clustering"
                    .into(),
            ));
        }
    }
    if let Some(mode) = &options.long_text_mode {
        if !matches!(mode.as_str(), "truncate" | "mean") {
            return Err(LlmError::Config(
                "Nomic :long-text-mode must be truncate or mean".into(),
            ));
        }
    }
    if options.truncate == Some(false) {
        return Err(LlmError::Config(
            "Nomic :truncate #f is unsupported; use :long-text-mode truncate or mean".into(),
        ));
    }
    if options.truncate == Some(true) && options.long_text_mode.as_deref() == Some("mean") {
        return Err(LlmError::Config(
            "Nomic :truncate conflicts with :long-text-mode mean".into(),
        ));
    }
    if options.input_type.is_some() || options.keep_alive.is_some() {
        return Err(LlmError::Config(
            "Nomic uses :task instead of :input-type and does not support :keep-alive".into(),
        ));
    }
    if options.dimensions == Some(0)
        || options.max_tokens_per_text == Some(0)
        || options.timeout_ms == Some(0)
    {
        return Err(LlmError::Config(
            "Nomic dimensions, token limit, and timeout must be positive".into(),
        ));
    }
    Ok(())
}

fn request_body(texts: &[String], model: &str, options: &EmbedOptions) -> Result<Value, LlmError> {
    validate_options(options)?;
    if model.trim().is_empty() {
        return Err(LlmError::Config(
            "Nomic embedding model must not be empty".into(),
        ));
    }
    let mut body = json!({"texts": texts, "model": model, "task_type": options.task.as_deref().unwrap_or("search_document")});
    if let Some(dimensions) = options.dimensions {
        body["dimensionality"] = json!(dimensions);
    }
    if let Some(mode) = options
        .long_text_mode
        .as_deref()
        .or(options.truncate.filter(|t| *t).map(|_| "truncate"))
    {
        body["long_text_mode"] = json!(mode);
    }
    if let Some(limit) = options.max_tokens_per_text {
        body["max_tokens_per_text"] = json!(limit);
    }
    Ok(body)
}

// Required fields follow the published response schema. Unknown fields are allowed
// so additive API changes do not make a valid response unreadable.
#[derive(Deserialize)]
struct NomicResponse {
    embeddings: Vec<Vec<f64>>,
    usage: NomicUsage,
    model: String,
}

#[derive(Deserialize)]
struct NomicUsage {
    prompt_tokens: u32,
    total_tokens: u32,
}

fn parse_response(
    body: &str,
    count: usize,
    model: &str,
    dimensions: Option<u32>,
) -> Result<EmbedResponse, LlmError> {
    let response: NomicResponse = serde_json::from_str(body)
        .map_err(|e| LlmError::Parse(format!("invalid Nomic embedding response: {e}")))?;
    let width = dimensions
        .map(|n| n as usize)
        .unwrap_or_else(|| response.embeddings.first().map_or(0, Vec::len));
    if response.model != model {
        return Err(LlmError::Parse(
            "Nomic returned a different embedding model".into(),
        ));
    }
    if response.embeddings.len() != count
        || width == 0
        || response
            .embeddings
            .iter()
            .any(|v| v.len() != width || v.iter().any(|n| !n.is_finite()))
    {
        return Err(LlmError::Parse(
            "Nomic must return one nonempty finite vector per text with matching dimensions".into(),
        ));
    }
    // Input tokens are a subset of total tokens. Only input usage is charged by
    // Sema's embedding accounting; this API has no completion-token field.
    if response.usage.total_tokens < response.usage.prompt_tokens {
        return Err(LlmError::Parse(
            "Nomic embedding usage has inconsistent token counts".into(),
        ));
    }
    Ok(EmbedResponse {
        embeddings: response.embeddings,
        usage: Usage {
            prompt_tokens: response.usage.prompt_tokens,
            model: response.model.clone(),
            ..Default::default()
        },
        model: response.model,
    })
}

impl LlmProvider for NomicEmbeddingProvider {
    fn name(&self) -> &str {
        "nomic"
    }
    fn default_model(&self) -> &str {
        &self.model
    }
    fn embedding_endpoint(&self) -> Option<&str> {
        Some(&self.base_url)
    }
    fn embedding_options(&self) -> EmbedOptions {
        self.options.clone()
    }
    fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Config(
            "Nomic does not support chat completions (embedding-only provider)".into(),
        ))
    }
    fn embed(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        sema_io::io_block_on(self.embed_async(request))
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn embed_future(&self, request: EmbedRequest) -> Option<crate::provider::BoxEmbedFuture<'_>> {
        Some(Box::pin(self.embed_async(request)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_request_preserves_api_long_text_defaults() {
        let body = request_body(
            &["hello".into()],
            "nomic-embed-text-v1.5",
            &EmbedOptions::default(),
        )
        .unwrap();
        assert_eq!(
            body,
            json!({"texts":["hello"],"model":"nomic-embed-text-v1.5","task_type":"search_document"})
        );
    }

    #[test]
    fn required_response_fields_preserve_usage() {
        let body = r#"{"embeddings":[[0.5,-0.5]],"model":"nomic-embed-text-v1.5","usage":{"prompt_tokens":23,"total_tokens":23},"extra":true}"#;
        let response = parse_response(body, 1, "nomic-embed-text-v1.5", Some(2)).unwrap();
        assert_eq!(response.usage.prompt_tokens, 23);
        assert_eq!(response.usage.completion_tokens, 0);
        assert_eq!(response.usage.total_tokens(), 23);
        for malformed in [
            body.replace("23", "23.5"),
            body.replace("0.5", "1e400"),
            body.replace("\"total_tokens\":23", "\"other\":23"),
        ] {
            assert!(parse_response(&malformed, 1, "nomic-embed-text-v1.5", None).is_err());
        }
    }

    #[test]
    fn truncate_alias_and_model_override() {
        let body = request_body(
            &[],
            "nomic-embed-text-v1",
            &EmbedOptions {
                truncate: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(body["long_text_mode"], "truncate");
        assert_eq!(body["model"], "nomic-embed-text-v1");
    }
}
