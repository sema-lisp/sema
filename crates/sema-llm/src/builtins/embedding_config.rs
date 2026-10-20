//! Shared retrieval-provider configuration for explicit and environment setup.
use super::*;

pub(super) fn string_option(
    opts: &BTreeMap<Value, Value>,
    name: &str,
) -> Result<Option<String>, SemaError> {
    opts.get(&Value::keyword(name))
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| SemaError::eval(format!("option :{name} must be a nonempty string")))
        })
        .transpose()
}

pub(super) fn provider_option(opts: &BTreeMap<Value, Value>) -> Result<Option<String>, SemaError> {
    opts.get(&Value::keyword("provider"))
        .map(|value| {
            value
                .as_keyword()
                .or_else(|| value.as_str().map(str::to_string))
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    SemaError::eval("option :provider must be a nonempty keyword or string")
                })
        })
        .transpose()
}

pub(super) fn build(
    name: &str,
    opts: &BTreeMap<Value, Value>,
) -> Result<(Box<dyn LlmProvider>, bool), SemaError> {
    use crate::embeddings::RerankDialect;
    let model = string_option(opts, "default-model")?.or(string_option(opts, "model")?);
    let base = string_option(opts, "base-url")?;
    let embedding_options = embed_options(opts)?;
    let top_k = opts
        .get(&Value::keyword("top-k"))
        .map(|v| {
            v.as_int()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| SemaError::eval("option :top-k must be a positive integer"))
        })
        .transpose()?;
    let rerank_model = string_option(opts, "rerank-model")?;
    if name == "ollama" {
        if rerank_model.is_some() {
            return Err(SemaError::eval("Ollama has no native reranking API"));
        }
        let host = base.or(string_option(opts, "host")?);
        let provider = OllamaProvider::new(host, None)
            .map_err(|e| SemaError::Llm(e.to_string()))?
            .with_embedding_model(model.unwrap_or_else(|| "embeddinggemma".to_string()))
            .with_embedding_options(embedding_options);
        return Ok((Box::new(provider), false));
    }
    let (key_env, default_url, default_model, rerank) = match name {
        "openai" => (
            "OPENAI_API_KEY",
            "https://api.openai.com/v1",
            "text-embedding-3-small",
            None,
        ),
        "jina" => (
            "JINA_API_KEY",
            "https://api.jina.ai/v1",
            "jina-embeddings-v3",
            Some(RerankDialect::Jina),
        ),
        "voyage" => (
            "VOYAGE_API_KEY",
            "https://api.voyageai.com/v1",
            "voyage-3",
            Some(RerankDialect::Voyage),
        ),
        "cohere" => (
            "COHERE_API_KEY",
            "https://api.cohere.com/v2",
            "embed-english-v3.0",
            None,
        ),
        "mistral" => (
            "MISTRAL_API_KEY",
            "https://api.mistral.ai/v1",
            "mistral-embed",
            None,
        ),
        "fireworks" => (
            "FIREWORKS_API_KEY",
            "https://api.fireworks.ai/inference/v1",
            "fireworks/qwen3-embedding-8b",
            Some(RerankDialect::Fireworks),
        ),
        "together" => (
            "TOGETHER_API_KEY",
            "https://api.together.ai/v1",
            "BAAI/bge-base-en-v1.5",
            Some(RerankDialect::Together),
        ),
        "gemini" => (
            "GEMINI_API_KEY",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-embedding-001",
            None,
        ),
        _ => ("", "", "text-embedding-3-small", None),
    };
    let key = match opts.get(&Value::keyword("api-key")) {
        Some(v) => v
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SemaError::type_error("string", v.type_name()))?,
        None => std::env::var(key_env)
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                (name == "gemini")
                    .then(|| std::env::var("GOOGLE_API_KEY").ok())
                    .flatten()
            })
            .unwrap_or_default(),
    };
    let base = base
        .or_else(|| {
            std::env::var(format!("{}_BASE_URL", name.to_ascii_uppercase()))
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| default_url.to_string());
    if base.is_empty() {
        return Err(SemaError::eval(format!(
            "unknown embedding provider '{name}': provide :base-url"
        )));
    }
    let model = model.unwrap_or_else(|| default_model.to_string());
    if name == "cohere" {
        let provider = CohereEmbeddingProvider::new(key, Some(model))
            .map_err(|e| SemaError::Llm(e.to_string()))?
            .with_base_url(base)
            .with_rerank_top_k(top_k)
            .with_rerank_model(rerank_model)
            .with_embedding_options(embedding_options);
        return Ok((Box::new(provider), true));
    }
    let mut provider = OpenAiCompatEmbeddingProvider::new(name.to_string(), key, base, model)
        .map_err(|e| SemaError::Llm(e.to_string()))?;
    if let Some(dialect) = rerank {
        provider = provider.with_rerank(dialect);
    }
    if rerank_model.is_some() && rerank.is_none() {
        return Err(SemaError::eval(format!(
            "provider '{name}' has no reranking API"
        )));
    }
    provider = provider
        .with_rerank_top_k(top_k)
        .with_rerank_model(rerank_model)
        .with_embedding_options(embedding_options);
    Ok((Box::new(provider), rerank.is_some()))
}

/// Parse shared embedding options without silently ignoring malformed values.
pub(super) fn embed_options(
    opts: &BTreeMap<Value, Value>,
) -> Result<crate::types::EmbedOptions, SemaError> {
    let positive = |key: &str| -> Result<Option<u64>, SemaError> {
        opts.get(&Value::keyword(key))
            .map(|v| {
                v.as_int()
                    .and_then(|n| u64::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        SemaError::eval(format!("option :{key} must be a positive integer"))
                    })
            })
            .transpose()
    };
    let dimensions = positive("dimensions")?
        .map(|n| u32::try_from(n).map_err(|_| SemaError::eval("option :dimensions is too large")))
        .transpose()?;
    let truncate = opts
        .get(&Value::keyword("truncate"))
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| SemaError::eval("option :truncate must be boolean"))
        })
        .transpose()?;
    let keep_alive = opts
        .get(&Value::keyword("keep-alive"))
        .map(|v| {
            if let Some(s) = v.as_str() {
                Ok(serde_json::json!(s))
            } else if let Some(n) = v.as_int() {
                Ok(serde_json::json!(n))
            } else {
                Err(SemaError::eval(
                    "option :keep-alive must be a duration string or integer",
                ))
            }
        })
        .transpose()?;
    Ok(crate::types::EmbedOptions {
        dimensions,
        input_type: string_option(opts, "input-type")?,
        task: string_option(opts, "task")?,
        truncate,
        keep_alive,
        timeout_ms: positive("timeout")?,
    })
}
