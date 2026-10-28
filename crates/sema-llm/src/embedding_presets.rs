//! Retrieval presets shared by explicit configuration and environment discovery.
use crate::embeddings::{EmbeddingDialect, RerankDialect};
use crate::types::LlmError;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderKind {
    Compatible,
    Cohere,
    Nomic,
    Ollama,
}

#[derive(Clone, Copy)]
enum Endpoint {
    Fixed(&'static str),
    Required,
    Azure,
    Cloudflare,
}

pub(crate) struct EmbeddingPreset {
    pub name: &'static str,
    pub kind: ProviderKind,
    pub dialect: EmbeddingDialect,
    pub rerank: Option<RerankDialect>,
    key_env: &'static [&'static str],
    base_env: &'static [&'static str],
    model_env: &'static [&'static str],
    endpoint: Endpoint,
    default_model: Option<&'static str>,
    require_key: bool,
}

impl EmbeddingPreset {
    const fn compatible(
        name: &'static str,
        key_env: &'static [&'static str],
        endpoint: &'static str,
        model: &'static str,
        dialect: EmbeddingDialect,
    ) -> Self {
        Self {
            name,
            kind: ProviderKind::Compatible,
            dialect,
            rerank: None,
            key_env,
            base_env: &[],
            model_env: &[],
            endpoint: Endpoint::Fixed(endpoint),
            default_model: Some(model),
            require_key: false,
        }
    }

    pub fn key(&self, env: &impl Fn(&str) -> Option<String>) -> Option<String> {
        first_env(self.key_env, env)
    }

    pub fn check_key(&self, key: &str) -> Result<(), LlmError> {
        if self.require_key && key.trim().is_empty() {
            return Err(LlmError::Config(format!(
                "{} requires :api-key or {}",
                self.name,
                self.key_env.join(" / ")
            )));
        }
        Ok(())
    }

    pub fn model(&self, env: &impl Fn(&str) -> Option<String>) -> Result<String, LlmError> {
        first_env(
            &[&format!(
                "{}_EMBEDDING_MODEL",
                self.name.to_ascii_uppercase()
            )],
            env,
        )
        .or_else(|| first_env(self.model_env, env))
        .or_else(|| self.default_model.map(str::to_string))
        .ok_or_else(|| {
            LlmError::Config(format!(
                "{} requires :model or {} (the deployed embedding model)",
                self.name,
                self.model_env.join(" / ")
            ))
        })
    }

    pub fn base_url(
        &self,
        explicit: Option<String>,
        account_id: Option<String>,
        env: &impl Fn(&str) -> Option<String>,
    ) -> Result<String, LlmError> {
        if let Some(base) = explicit
            .or_else(|| {
                first_env(
                    &[&format!("{}_BASE_URL", self.name.to_ascii_uppercase())],
                    env,
                )
            })
            .or_else(|| first_env(self.base_env, env))
        {
            return Ok(base.trim_end_matches('/').to_string());
        }
        match self.endpoint {
            Endpoint::Fixed(url) => Ok(url.to_string()),
            Endpoint::Azure => {
                let endpoint = first_env(&["AZURE_OPENAI_ENDPOINT"], env).ok_or_else(|| {
                    LlmError::Config(
                        "azure requires :base-url, AZURE_BASE_URL, or AZURE_OPENAI_ENDPOINT".into(),
                    )
                })?;
                let endpoint = endpoint.trim_end_matches('/');
                Ok(if endpoint.ends_with("/openai/v1") {
                    endpoint.to_string()
                } else {
                    format!("{endpoint}/openai/v1")
                })
            }
            Endpoint::Cloudflare => {
                let account = account_id
                    .or_else(|| first_env(&["CLOUDFLARE_ACCOUNT_ID"], env))
                    .ok_or_else(|| {
                        LlmError::Config(
                            "cloudflare requires :base-url or :account-id / CLOUDFLARE_ACCOUNT_ID"
                                .into(),
                        )
                    })?;
                if account.is_empty() || !account.bytes().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(LlmError::Config(
                        "Cloudflare account ID must contain only ASCII letters and digits".into(),
                    ));
                }
                Ok(format!(
                    "https://api.cloudflare.com/client/v4/accounts/{account}/ai/v1"
                ))
            }
            Endpoint::Required => Err(LlmError::Config(format!(
                "{} requires :base-url or {}_BASE_URL for the account's region/workspace",
                self.name,
                self.name.to_ascii_uppercase()
            ))),
        }
    }

    pub fn supports_rerank(&self) -> bool {
        self.rerank.is_some() || self.kind == ProviderKind::Cohere
    }

    pub fn auto_credentials_available(&self, env: &impl Fn(&str) -> Option<String>) -> bool {
        self.kind == ProviderKind::Ollama
            || self.dialect == EmbeddingDialect::Tei
            || self.key(env).is_some()
    }

    /// Partial credentials for an unselected service must not break chat startup.
    pub fn auto_available(&self, env: &impl Fn(&str) -> Option<String>) -> bool {
        if self.kind == ProviderKind::Ollama {
            return true;
        }
        self.auto_credentials_available(env)
            && self.base_url(None, None, env).is_ok()
            && self.model(env).is_ok()
    }
}

fn first_env(names: &[&str], env: &impl Fn(&str) -> Option<String>) -> Option<String> {
    names
        .iter()
        .find_map(|name| env(name).filter(|value| !value.trim().is_empty()))
}

pub(crate) fn environment(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub(crate) fn find(name: &str) -> Option<&'static EmbeddingPreset> {
    PRESETS.iter().find(|preset| preset.name == name)
}

// Order is the automatic embedding preference. Keep existing hosted providers
// ahead of new presets and retain Ollama as the no-credential fallback.
pub(crate) const PRESETS: &[EmbeddingPreset] = &[
    EmbeddingPreset {
        rerank: Some(RerankDialect::Jina),
        ..EmbeddingPreset::compatible(
            "jina",
            &["JINA_API_KEY"],
            "https://api.jina.ai/v1",
            "jina-embeddings-v3",
            EmbeddingDialect::Jina,
        )
    },
    EmbeddingPreset {
        rerank: Some(RerankDialect::Voyage),
        ..EmbeddingPreset::compatible(
            "voyage",
            &["VOYAGE_API_KEY"],
            "https://api.voyageai.com/v1",
            "voyage-3",
            EmbeddingDialect::Voyage,
        )
    },
    EmbeddingPreset {
        kind: ProviderKind::Cohere,
        ..EmbeddingPreset::compatible(
            "cohere",
            &["COHERE_API_KEY"],
            "https://api.cohere.com/v2",
            "embed-english-v3.0",
            EmbeddingDialect::Compatible,
        )
    },
    EmbeddingPreset {
        kind: ProviderKind::Nomic,
        ..EmbeddingPreset::compatible(
            "nomic",
            &["NOMIC_API_KEY"],
            "https://api-atlas.nomic.ai/v1",
            "nomic-embed-text-v1.5",
            EmbeddingDialect::Compatible,
        )
    },
    EmbeddingPreset::compatible(
        "openai",
        &["OPENAI_API_KEY"],
        "https://api.openai.com/v1",
        "text-embedding-3-small",
        EmbeddingDialect::OpenAi,
    ),
    EmbeddingPreset::compatible(
        "mistral",
        &["MISTRAL_API_KEY"],
        "https://api.mistral.ai/v1",
        "mistral-embed",
        EmbeddingDialect::Mistral,
    ),
    EmbeddingPreset {
        rerank: Some(RerankDialect::Fireworks),
        ..EmbeddingPreset::compatible(
            "fireworks",
            &["FIREWORKS_API_KEY"],
            "https://api.fireworks.ai/inference/v1",
            "fireworks/qwen3-embedding-8b",
            EmbeddingDialect::Compatible,
        )
    },
    EmbeddingPreset {
        rerank: Some(RerankDialect::Together),
        ..EmbeddingPreset::compatible(
            "together",
            &["TOGETHER_API_KEY"],
            "https://api.together.ai/v1",
            "BAAI/bge-base-en-v1.5",
            EmbeddingDialect::Compatible,
        )
    },
    EmbeddingPreset::compatible(
        "gemini",
        &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "gemini-embedding-001",
        EmbeddingDialect::OpenAi,
    ),
    EmbeddingPreset {
        require_key: true,
        ..EmbeddingPreset::compatible(
            "openrouter",
            &["OPENROUTER_API_KEY"],
            "https://openrouter.ai/api/v1",
            "openai/text-embedding-3-small",
            EmbeddingDialect::OpenAi,
        )
    },
    EmbeddingPreset {
        require_key: true,
        ..EmbeddingPreset::compatible(
            "deepinfra",
            &["DEEPINFRA_API_KEY"],
            "https://api.deepinfra.com/v1/openai",
            "Qwen/Qwen3-Embedding-0.6B",
            EmbeddingDialect::OpenAi,
        )
    },
    EmbeddingPreset {
        endpoint: Endpoint::Azure,
        default_model: None,
        require_key: true,
        base_env: &["AZURE_OPENAI_BASE_URL"],
        model_env: &["AZURE_OPENAI_EMBEDDING_DEPLOYMENT"],
        ..EmbeddingPreset::compatible(
            "azure",
            &["AZURE_OPENAI_API_KEY", "AZURE_API_KEY"],
            "",
            "",
            EmbeddingDialect::OpenAi,
        )
    },
    EmbeddingPreset {
        endpoint: Endpoint::Cloudflare,
        require_key: true,
        ..EmbeddingPreset::compatible(
            "cloudflare",
            &["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_API_KEY"],
            "",
            "@cf/baai/bge-m3",
            EmbeddingDialect::Cloudflare,
        )
    },
    EmbeddingPreset {
        endpoint: Endpoint::Required,
        require_key: true,
        base_env: &["DASHSCOPE_BASE_URL"],
        ..EmbeddingPreset::compatible(
            "alibaba",
            &["ALIBABA_API_KEY", "DASHSCOPE_API_KEY"],
            "",
            "text-embedding-v4",
            EmbeddingDialect::Alibaba,
        )
    },
    EmbeddingPreset {
        default_model: None,
        model_env: &["TEI_MODEL"],
        ..EmbeddingPreset::compatible(
            "tei",
            &["TEI_API_KEY"],
            "http://127.0.0.1:8080/v1",
            "",
            EmbeddingDialect::Tei,
        )
    },
    EmbeddingPreset {
        kind: ProviderKind::Ollama,
        ..EmbeddingPreset::compatible(
            "ollama",
            &[],
            "http://localhost:11434",
            "embeddinggemma",
            EmbeddingDialect::Compatible,
        )
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(values: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn account_endpoints_and_overrides() {
        let azure = find("azure").unwrap();
        for root in [
            "https://example.openai.azure.com/",
            "https://example.openai.azure.com/openai/v1/",
        ] {
            assert_eq!(
                azure
                    .base_url(None, None, &env(&[("AZURE_OPENAI_ENDPOINT", root)]))
                    .unwrap(),
                "https://example.openai.azure.com/openai/v1"
            );
        }
        let cf = find("cloudflare").unwrap();
        assert_eq!(
            cf.base_url(None, None, &env(&[("CLOUDFLARE_ACCOUNT_ID", "abc123")]))
                .unwrap(),
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/v1"
        );
        assert!(cf.base_url(None, Some("../bad".into()), &env(&[])).is_err());
        for name in ["azure", "cloudflare", "alibaba"] {
            let preset = find(name).unwrap();
            assert!(preset.base_url(None, None, &env(&[])).is_err());
            assert_eq!(
                preset
                    .base_url(Some("http://localhost:9000/v1/".into()), None, &env(&[]))
                    .unwrap(),
                "http://localhost:9000/v1"
            );
        }
    }

    #[test]
    fn models_and_partial_accounts() {
        let azure = find("azure").unwrap();
        assert!(azure.model(&env(&[])).is_err());
        assert_eq!(
            azure
                .model(&env(&[(
                    "AZURE_OPENAI_EMBEDDING_DEPLOYMENT",
                    "my-deployment"
                )]))
                .unwrap(),
            "my-deployment"
        );
        assert!(!azure.auto_available(&env(&[("AZURE_OPENAI_API_KEY", "test")])));
        assert!(!find("cloudflare")
            .unwrap()
            .auto_available(&env(&[("CLOUDFLARE_API_TOKEN", "test")])));
        assert!(!find("tei").unwrap().auto_available(&env(&[])));
        assert!(find("tei")
            .unwrap()
            .auto_available(&env(&[("TEI_MODEL", "served-model")])));
        assert!(find("ollama").unwrap().auto_available(&env(&[])));
    }

    #[test]
    fn aliases_priority_and_unique_names() {
        assert_eq!(
            find("alibaba")
                .unwrap()
                .key(&env(&[("DASHSCOPE_API_KEY", "alias")])),
            Some("alias".into())
        );
        assert_eq!(
            find("gemini")
                .unwrap()
                .key(&env(&[("GEMINI_API_KEY", ""), ("GOOGLE_API_KEY", "alias")])),
            Some("alias".into())
        );
        let names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
        assert_eq!(
            &names[..9],
            &[
                "jina",
                "voyage",
                "cohere",
                "nomic",
                "openai",
                "mistral",
                "fireworks",
                "together",
                "gemini"
            ]
        );
        assert_eq!(names.last(), Some(&"ollama"));
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
    }
}
