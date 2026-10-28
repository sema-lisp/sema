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
    use crate::embedding_presets::{self, environment, ProviderKind};
    let preset = embedding_presets::find(name);
    let kind = preset.map_or(ProviderKind::Compatible, |p| p.kind);
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
    let model = match model {
        Some(model) => model,
        None => match preset {
            Some(preset) => preset
                .model(&environment)
                .map_err(|e| SemaError::Llm(e.to_string()))?,
            None => "text-embedding-3-small".to_string(),
        },
    };
    if kind == ProviderKind::Ollama {
        if rerank_model.is_some() {
            return Err(SemaError::eval("Ollama has no native reranking API"));
        }
        let host = base.or(string_option(opts, "host")?);
        let provider = OllamaProvider::new(host, None)
            .map_err(|e| SemaError::Llm(e.to_string()))?
            .with_embedding_model(model)
            .with_embedding_options(embedding_options);
        return Ok((Box::new(provider), false));
    }
    let rerank = preset.and_then(|preset| preset.rerank);
    let key = match opts.get(&Value::keyword("api-key")) {
        Some(v) => v
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SemaError::type_error("string", v.type_name()))?,
        None => preset
            .and_then(|preset| preset.key(&environment))
            .unwrap_or_default(),
    };
    if let Some(preset) = preset {
        preset
            .check_key(&key)
            .map_err(|e| SemaError::Llm(e.to_string()))?;
    }
    let base = match preset {
        Some(preset) => preset
            .base_url(base, string_option(opts, "account-id")?, &environment)
            .map_err(|e| SemaError::Llm(e.to_string()))?,
        None => base
            .or_else(|| {
                std::env::var(format!("{}_BASE_URL", name.to_ascii_uppercase()))
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .ok_or_else(|| {
                SemaError::eval(format!(
                    "unknown embedding provider '{name}': provide :base-url"
                ))
            })?,
    };
    if kind == ProviderKind::Nomic {
        if rerank_model.is_some() || top_k.is_some() {
            return Err(SemaError::eval(
                "Nomic has no configured hosted reranking API",
            ));
        }
        let provider =
            crate::nomic::NomicEmbeddingProvider::new(key, base, model, embedding_options)
                .map_err(|e| SemaError::Llm(e.to_string()))?;
        return Ok((Box::new(provider), false));
    }
    if kind == ProviderKind::Cohere {
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
    if (rerank_model.is_some() || top_k.is_some()) && rerank.is_none() {
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
        long_text_mode: string_option(opts, "long-text-mode")?,
        max_tokens_per_text: positive("max-tokens-per-text")?
            .map(|n| {
                u32::try_from(n)
                    .map_err(|_| SemaError::eval("option :max-tokens-per-text is too large"))
            })
            .transpose()?,
        truncate,
        keep_alive,
        timeout_ms: positive("timeout")?,
    })
}
