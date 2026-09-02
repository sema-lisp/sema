use super::*;

/// Extract the host from a provider `base-url`/`host` string without pulling in
/// a URL-parsing dependency. Handles `scheme://`, userinfo, `[ipv6]`, and ports.
pub(super) fn url_host(url: &str) -> Option<String> {
    let after = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(rest) = hostport.strip_prefix('[') {
        // [::1]:port -> ::1
        return rest.split(']').next().map(|s| s.to_string());
    }
    hostport.split(':').next().map(|s| s.to_string())
}

/// True if `host` points at the local machine or a private/internal network —
/// the targets an SSRF would pivot to. Used to reject attacker-chosen provider
/// `base-url`s when running untrusted (sandboxed) code.
pub(super) fn is_internal_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    if h.is_empty() || h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => ipv4_is_internal(v4),
        Ok(std::net::IpAddr::V6(v6)) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // IPv4-mapped (::ffff:a.b.c.d) — re-check against v4 rules.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ipv4_is_internal(v4);
            }
            let seg0 = v6.segments()[0];
            (seg0 & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (seg0 & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
        // `IpAddr::parse` only accepts canonical dotted-decimal, but
        // `getaddrinfo` (what reqwest ultimately calls) also accepts the
        // inet_aton forms: decimal (`2130706433`), octal (`0177.0.0.1`),
        // hex (`0x7f.0.0.1`), and short (`127.1`). Decode those and re-check,
        // so e.g. `http://2130706433/` can't smuggle loopback past the gate.
        Err(_) => parse_loose_ipv4(&h).map(ipv4_is_internal).unwrap_or(false),
    }
}

/// Internal/private/loopback test shared by every IPv4 path.
pub(super) fn ipv4_is_internal(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.octets()[0] == 0
}

/// Parse the loose `inet_aton` IPv4 forms that `getaddrinfo` accepts but
/// `Ipv4Addr::from_str` rejects: 1–4 dot-separated parts, each decimal,
/// octal (`0` prefix), or hex (`0x` prefix); a trailing dot is allowed.
/// Returns `None` for anything that isn't such a numeric address (i.e. a real
/// hostname), so non-numeric public hosts fall through to "not internal".
pub(super) fn parse_loose_ipv4(host: &str) -> Option<std::net::Ipv4Addr> {
    let host = host.strip_suffix('.').unwrap_or(host);
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let nums: Vec<u32> = parts
        .iter()
        .map(|p| parse_uint_part(p))
        .collect::<Option<Vec<_>>>()?;
    let addr: u32 = match nums.as_slice() {
        [a] => *a,
        [a, b] if *a <= 0xff && *b <= 0x00ff_ffff => (a << 24) | b,
        [a, b, c] if *a <= 0xff && *b <= 0xff && *c <= 0xffff => (a << 24) | (b << 16) | c,
        [a, b, c, d] if [a, b, c, d].iter().all(|x| **x <= 0xff) => {
            (a << 24) | (b << 16) | (c << 8) | d
        }
        _ => return None, // a part overflowed its field — not a valid packed address
    };
    Some(std::net::Ipv4Addr::from(addr))
}

/// Parse a single inet_aton numeric part: hex (`0x..`), octal (`0..`), decimal.
pub(super) fn parse_uint_part(s: &str) -> Option<u32> {
    if let Some(hex) = s.strip_prefix("0x") {
        if hex.is_empty() {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    } else if s.len() > 1 && s.starts_with('0') {
        u32::from_str_radix(&s[1..], 8).ok()
    } else {
        s.parse::<u32>().ok()
    }
}

/// Reject provider URLs that target internal hosts when running sandboxed.
/// Trusted (unrestricted) sessions — the normal CLI/REPL/notebook — keep full
/// access so local proxies and Ollama on `localhost` continue to work.
pub(super) fn guard_provider_url(
    unrestricted: bool,
    opts: &BTreeMap<Value, Value>,
) -> Result<(), SemaError> {
    if unrestricted {
        return Ok(());
    }
    let url = opts.opt_str("base-url").or_else(|| opts.opt_str("host"));
    if let Some(url) = url {
        if url_host(&url).is_some_and(|h| is_internal_host(&h)) {
            return Err(SemaError::eval(format!(
                "llm/configure: base-url '{url}' targets an internal/loopback host, \
                 which is not allowed under the current sandbox"
            ))
            .with_hint(
                "grant the network capability and run unsandboxed to use a local endpoint",
            ));
        }
    }
    Ok(())
}

pub(super) fn complete_with_prompt(
    prompt: &Prompt,
    opts: Option<&Value>,
) -> sema_core::runtime::NativeResult {
    #[allow(unused_imports)]
    use sema_core::runtime::NativeOutcome;
    let messages: Vec<ChatMessage> = prompt
        .messages
        .iter()
        .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
        .collect();

    let mut model = String::new();
    let mut max_tokens = None;
    let mut temperature = None;

    if let Some(opts) = opts.and_then(|v| v.as_map_rc()) {
        model = opts.opt_str("model").unwrap_or_default();
        max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
        temperature = opts.opt_f64("temperature");
    }

    let mut request = ChatRequest::new(model, messages);
    request.max_tokens = max_tokens.or(Some(4096));
    request.temperature = temperature;
    request.timeout_ms = opt_timeout_ms(opts);

    // Per-call observability tags/metadata (read inside do_complete's span).
    let _tele = install_call_telemetry(opts.and_then(|v| v.as_map_rc()).as_ref());

    // Shared by `llm/send` and the Prompt-arg branch of `llm/complete`: runtime
    // roots and spawned tasks suspend on an External wait; host calls are synchronous.
    #[cfg(not(target_arch = "wasm32"))]
    {
        dispatch_complete_offload(
            request,
            CompleteFinalize::new(|resp| Ok(Value::string(&resp.content))),
        )
    }
    #[cfg(target_arch = "wasm32")]
    {
        let response = do_complete(request)?;
        track_usage(&response.usage)?;
        Ok(NativeOutcome::Return(Value::string(&response.content)))
    }
}

pub(super) fn register(env: &Env, sandbox: &sema_core::Sandbox, unrestricted: bool) {
    // (llm/configure :anthropic {:api-key "..." :default-model "..."})
    // (llm/configure :openai {:api-key "..." :base-url "..." :default-model "..."})
    register_fn(env, "llm/configure", move |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/configure", "2", args.len()));
        }
        let provider_name = args.keyword_at(0, "llm/configure")?;
        let opts_rc = args.map_at(1, "llm/configure")?;
        let opts = opts_rc.as_ref().clone();

        guard_provider_url(unrestricted, &opts)?;

        let api_key = opts.opt_str("api-key");

        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            match provider_name.as_str() {
                "anthropic" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model");
                    let provider = AnthropicProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("anthropic");
                }
                "openai" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let base_url = opts.opt_str("base-url");
                    let model = opts.opt_str("default-model");
                    let provider = OpenAiProvider::new(api_key, base_url, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("openai");
                }
                "gemini" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model");
                    let provider = GeminiProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("gemini");
                }
                "groq" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model")
                        .unwrap_or_else(|| "llama-3.3-70b-versatile".to_string());
                    let base_url = opts.opt_str("base-url")
                        .unwrap_or_else(|| "https://api.groq.com/openai/v1".to_string());
                    let provider =
                        OpenAiProvider::named("groq".to_string(), api_key, base_url, model, true)
                            .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("groq");
                }
                "xai" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model")
                        .unwrap_or_else(|| "grok-4.3".to_string());
                    let base_url = opts.opt_str("base-url")
                        .unwrap_or_else(|| "https://api.x.ai/v1".to_string());
                    let provider =
                        OpenAiProvider::named("xai".to_string(), api_key, base_url, model, true)
                            .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("xai");
                }
                "mistral" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model")
                        .unwrap_or_else(|| "mistral-large-latest".to_string());
                    let base_url = opts.opt_str("base-url")
                        .unwrap_or_else(|| "https://api.mistral.ai/v1".to_string());
                    let provider = OpenAiProvider::named(
                        "mistral".to_string(),
                        api_key,
                        base_url,
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("mistral");
                }
                "moonshot" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model")
                        .unwrap_or_else(|| "kimi-k2.6".to_string());
                    let base_url = opts.opt_str("base-url")
                        .unwrap_or_else(|| "https://api.moonshot.ai/v1".to_string());
                    let provider = OpenAiProvider::named(
                        "moonshot".to_string(),
                        api_key,
                        base_url,
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("moonshot");
                }
                "ollama" => {
                    let host =
                        opts.opt_str("host").or_else(|| opts.opt_str("base-url"));
                    let model = opts.opt_str("default-model");
                    // Ollama doesn't use api-key
                    let provider = OllamaProvider::new(host, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("ollama");
                }
                "jina" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model")
                        .unwrap_or_else(|| "jina-embeddings-v3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        api_key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Jina);
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                    reg.set_rerank_provider("jina");
                }
                "voyage" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model")
                        .unwrap_or_else(|| "voyage-3-lite".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        api_key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Voyage);
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("voyage");
                    reg.set_rerank_provider("voyage");
                }
                "cohere" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model");
                    let provider = CohereEmbeddingProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("cohere");
                    reg.set_rerank_provider("cohere");
                }
                other => {
                    // Treat unknown providers as OpenAI-compatible if base-url and api-key are provided
                    let api_key = api_key.clone().ok_or_else(|| {
                        SemaError::Llm(format!(
                            "unknown provider '{other}': provide :api-key and :base-url to register as OpenAI-compatible"
                        ))
                    })?;
                    let base_url = opts.opt_str("base-url").ok_or_else(|| {
                        SemaError::Llm(format!(
                            "unknown provider '{other}': provide :base-url to register as OpenAI-compatible"
                        ))
                    })?;
                    let model = opts.opt_str("default-model")
                        .unwrap_or_else(|| "default".to_string());
                    let provider = OpenAiProvider::named(
                        other.to_string(),
                        api_key,
                        base_url,
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default(other);
                }
            }
            Ok(Value::nil())
        })
    });

    // (llm/define-provider :name {:complete fn :default-model "..." :stream fn})
    register_fn(env, "llm/define-provider", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/define-provider", "2", args.len()));
        }
        let provider_name = args.keyword_at(0, "llm/define-provider")?;
        let opts_rc = args.map_at(1, "llm/define-provider")?;
        let opts = opts_rc.as_ref().clone();

        let complete_fn = opts
            .get(&Value::keyword("complete"))
            .cloned()
            .ok_or_else(|| SemaError::eval("llm/define-provider requires :complete function"))?;

        if complete_fn.as_lambda_rc().is_none() && complete_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", complete_fn.type_name()));
        }

        let default_model = opts
            .opt_str("default-model")
            .unwrap_or_else(|| "default".to_string());

        let name_for_callbacks = provider_name.clone();
        LISP_PROVIDERS.with(|providers| {
            providers
                .borrow_mut()
                .insert(name_for_callbacks, LispProviderCallbacks { complete_fn });
        });

        let name_for_registry = provider_name.clone();
        let model_clone = default_model.clone();
        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            reg.register(Box::new(LispProvider {
                name: name_for_registry,
                default_model: model_clone,
            }));
            reg.set_default(&provider_name);
        });

        Ok(Value::keyword(&provider_name))
    });

    // Auto-configure from environment variables
    register_fn(env, "llm/auto-configure", |_args| {
        // New scoped env vars (preferred)
        let chat_model = std::env::var("SEMA_CHAT_MODEL")
            .ok()
            .filter(|m| !m.is_empty());
        let chat_provider = std::env::var("SEMA_CHAT_PROVIDER")
            .ok()
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty());
        let embedding_model = std::env::var("SEMA_EMBEDDING_MODEL")
            .ok()
            .filter(|m| !m.is_empty());
        let embedding_provider = std::env::var("SEMA_EMBEDDING_PROVIDER")
            .ok()
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty());

        let forced_chat_model = chat_model;
        let forced_chat_provider = chat_provider;

        let result = PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            let mut first_configured: Option<String> = None;

            // Determine which provider gets the chat model override.
            // Only the provider that will become the default chat provider should
            // receive forced_chat_model — not every provider.
            let target_chat_provider = forced_chat_provider.as_deref();

            // Inline to avoid borrow conflicts with first_configured.
            macro_rules! model_for {
                ($name:expr) => {{
                    match target_chat_provider {
                        Some(target) if target == $name => forced_chat_model.clone(),
                        None if first_configured.is_none() => forced_chat_model.clone(),
                        _ => None,
                    }
                }};
            }

            // Try Anthropic first (preferred)
            if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                if !key.is_empty() {
                    let provider = AnthropicProvider::new(key, model_for!("anthropic"))
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("anthropic");
                        first_configured = Some("anthropic".to_string());
                    }
                }
            }
            // Try OpenAI
            if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                if !key.is_empty() {
                    let provider = OpenAiProvider::new(key, None, model_for!("openai"))
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("openai");
                        first_configured = Some("openai".to_string());
                    }
                }
            }
            // Try Groq
            if let Ok(key) = std::env::var("GROQ_API_KEY") {
                if !key.is_empty() {
                    let model =
                        model_for!("groq").unwrap_or_else(|| "llama-3.3-70b-versatile".to_string());
                    let provider = OpenAiProvider::named(
                        "groq".to_string(),
                        key,
                        "https://api.groq.com/openai/v1".to_string(),
                        model,
                        true,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("groq");
                        first_configured = Some("groq".to_string());
                    }
                }
            }
            // Try xAI
            if let Ok(key) = std::env::var("XAI_API_KEY") {
                if !key.is_empty() {
                    let model = model_for!("xai").unwrap_or_else(|| "grok-4.3".to_string());
                    let provider = OpenAiProvider::named(
                        "xai".to_string(),
                        key,
                        "https://api.x.ai/v1".to_string(),
                        model,
                        true,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("xai");
                        first_configured = Some("xai".to_string());
                    }
                }
            }
            // Try Mistral
            if let Ok(key) = std::env::var("MISTRAL_API_KEY") {
                if !key.is_empty() {
                    let model =
                        model_for!("mistral").unwrap_or_else(|| "mistral-large-latest".to_string());
                    let provider = OpenAiProvider::named(
                        "mistral".to_string(),
                        key,
                        "https://api.mistral.ai/v1".to_string(),
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("mistral");
                        first_configured = Some("mistral".to_string());
                    }
                }
            }
            // Try Moonshot
            if let Ok(key) = std::env::var("MOONSHOT_API_KEY") {
                if !key.is_empty() {
                    let model = model_for!("moonshot").unwrap_or_else(|| "kimi-k2.6".to_string());
                    let provider = OpenAiProvider::named(
                        "moonshot".to_string(),
                        key,
                        "https://api.moonshot.ai/v1".to_string(),
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("moonshot");
                        first_configured = Some("moonshot".to_string());
                    }
                }
            }
            // Try Google Gemini
            if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
                if !key.is_empty() {
                    let provider = GeminiProvider::new(key, model_for!("gemini"))
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("gemini");
                        first_configured = Some("gemini".to_string());
                    }
                }
            }
            // Ollama (local, no auth) — always register; defaults to http://localhost:11434
            {
                let provider = OllamaProvider::new(None, model_for!("ollama"))
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                reg.register(Box::new(provider));
                if first_configured.is_none() {
                    reg.set_default("ollama");
                    first_configured = Some("ollama".to_string());
                }
            }

            // Auto-configure embedding providers
            // Determine the embedding model override for the target embedding provider.
            // If --embedding-provider is set, only that provider gets the model override.
            // Otherwise, the first successfully configured embedding provider gets it.
            let target_embed_provider = embedding_provider.as_deref();

            // Helper: should this embedding provider get the model override?
            // Inline to avoid borrow conflicts with reg.
            macro_rules! embed_model_for {
                ($name:expr, $default:expr) => {{
                    let model_override = match target_embed_provider {
                        Some(target) if target == $name => embedding_model.clone(),
                        None if reg.embedding_provider().is_none() => embedding_model.clone(),
                        _ => None,
                    };
                    model_override.unwrap_or_else(|| $default.to_string())
                }};
            }

            if let Ok(key) = std::env::var("JINA_API_KEY") {
                if !key.is_empty() {
                    let model = embed_model_for!("jina", "jina-embeddings-v3");
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Jina);
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                    reg.set_rerank_provider("jina");
                }
            }
            if let Ok(key) = std::env::var("VOYAGE_API_KEY") {
                if !key.is_empty() {
                    let model = embed_model_for!("voyage", "voyage-3");
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Voyage);
                    reg.register(Box::new(provider));
                    // Only set as embedding provider if not already set
                    if reg.embedding_provider().is_none() {
                        reg.set_embedding_provider("voyage");
                    }
                    if reg.rerank_provider().is_none() {
                        reg.set_rerank_provider("voyage");
                    }
                }
            }
            if let Ok(key) = std::env::var("COHERE_API_KEY") {
                if !key.is_empty() {
                    let model_override = match target_embed_provider {
                        Some("cohere") => embedding_model.clone(),
                        None if reg.embedding_provider().is_none() => embedding_model.clone(),
                        _ => None,
                    };
                    let provider = CohereEmbeddingProvider::new(key, model_override)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if reg.embedding_provider().is_none() {
                        reg.set_embedding_provider("cohere");
                    }
                    if reg.rerank_provider().is_none() {
                        reg.set_rerank_provider("cohere");
                    }
                }
            }
            // Fallback: use OpenAI for embeddings if no dedicated provider was configured.
            // Use a distinct name to avoid overwriting the OpenAI chat provider.
            if reg.embedding_provider().is_none() {
                if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                    if !key.is_empty() {
                        let model = embed_model_for!("openai", "text-embedding-3-small");
                        let provider = OpenAiCompatEmbeddingProvider::new(
                            "openai-embeddings".to_string(),
                            key,
                            "https://api.openai.com/v1".to_string(),
                            model,
                        )
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                        reg.register(Box::new(provider));
                        reg.set_embedding_provider("openai-embeddings");
                    }
                }
            }

            // Apply forced chat provider override
            if let Some(requested_provider) = forced_chat_provider.as_deref() {
                if reg.get(requested_provider).is_some() {
                    reg.set_default(requested_provider);
                    first_configured = Some(requested_provider.to_string());
                } else {
                    return Err(SemaError::Llm(format!(
                        "requested provider is not configured: {requested_provider}"
                    )));
                }
            }

            // Apply forced embedding provider override
            if let Some(requested_embed) = target_embed_provider {
                if reg.get(requested_embed).is_some() {
                    reg.set_embedding_provider(requested_embed);
                } else {
                    return Err(SemaError::Llm(format!(
                        "requested embedding provider is not configured: {requested_embed}"
                    )));
                }
            }

            match first_configured {
                Some(name) => Ok(Value::keyword(&name)),
                None => Ok(Value::nil()),
            }
        })?;

        Ok(result)
    });

    // (llm/complete "prompt text" {:model "..." :max-tokens 200 :temperature 0.5})
    register_runtime_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/complete", |args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/complete", "1-2", args.len()));
        }
        let prompt_text = if let Some(p) = args[0].as_prompt_rc() {
            return complete_with_prompt(&p, args.get(1));
        } else if let Some(s) = args[0].as_str() {
            s.to_string()
        } else {
            return Err(SemaError::type_error(
                "string or prompt",
                args[0].type_name(),
            ));
        };

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;
        let mut reasoning_effort = None;
        let mut conv_scope = ConvScope::default();

        if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                conv_scope = ConvScope::from_opts(Some(&opts));
                model = opts.opt_str("model").unwrap_or_default();
                max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
                temperature = opts.opt_f64("temperature");
                system = opts.opt_str("system");
                reasoning_effort = opts.opt_name("reasoning-effort");
            }
        }

        // Honor a caller-supplied conversation/session/user identity (else do_complete
        // generates a fresh conversation id).
        let _conv = conv_scope.open();
        // Per-call observability tags/metadata (read inside do_complete's span).
        let _tele = install_call_telemetry(args.get(1).and_then(|v| v.as_map_rc()).as_ref());

        let messages = vec![ChatMessage::new("user", prompt_text)];

        let mut request = ChatRequest::new(model, messages);
        request.max_tokens = max_tokens.or(Some(4096));
        request.temperature = temperature;
        request.system = system;
        request.reasoning_effort = reasoning_effort;
        request.timeout_ms = opt_timeout_ms(args.get(1));

        // In any runtime quantum, including root-main, the completion driver calls
        // Sema-defined providers structurally on the VM and suspends each native
        // provider attempt on an External wait. Finalization accounts usage and
        // shapes the value after either kind of provider succeeds.
        #[cfg(not(target_arch = "wasm32"))]
        {
            dispatch_complete_offload(
                request,
                CompleteFinalize::new(|resp| Ok(Value::string(&resp.content))),
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let response = do_complete(request)?;
            track_usage(&response.usage)?;
            Ok(NativeOutcome::Return(Value::string(&response.content)))
        }
    });

    // (llm/chat messages {:model "..." :tools [...] :tool-mode :auto ...})
    // Synchronous / no-tools-needed twin. The Sema-visible `llm/chat` is a prelude
    // dispatcher (mirrors `agent/run`): in a runtime quantum with a configured tool
    // loop it drives `__chat-begin` + the shared `__agent-*` step natives instead
    // (a native cannot retain a Rust loop across multiple suspensions); every
    // other case—top level, or no `:tools`/`:tool-mode :none`—reaches this native.
    // Its runtime branch offloads the no-tools completion. Gated as "llm/chat" (not this native's own
    // registration name) so a sandboxed caller sees the same `PermissionDenied`
    // regardless of which internal entry point actually runs.
    register_runtime_fn_ctx_gated_as(
        env,
        sandbox,
        sema_core::Caps::LLM,
        "__llm-chat-blocking",
        "llm/chat",
        |ctx, args| {
            #[allow(unused_imports)]
            use sema_core::runtime::NativeOutcome;
            if args.is_empty() || args.len() > 2 {
                return Err(SemaError::arity("llm/chat", "1-2", args.len()));
            }

            let messages = extract_messages(&args[0])?;

            let mut model = String::new();
            let mut max_tokens = None;
            let mut temperature = None;
            let mut system = None;
            let mut reasoning_effort = None;
            let mut tools: Vec<Value> = Vec::new();
            let mut tool_mode = "auto".to_string();
            let mut max_tool_rounds = 10usize;
            let mut on_tool_call: Option<Value> = None;
            let mut conv_scope = ConvScope::default();

            if let Some(opts_val) = args.get(1) {
                if let Some(opts) = opts_val.as_map_rc() {
                    conv_scope = ConvScope::from_opts(Some(&opts));
                    model = opts.opt_str("model").unwrap_or_default();
                    max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
                    temperature = opts.opt_f64("temperature");
                    system = opts.opt_str("system");
                    reasoning_effort = opts.opt_name("reasoning-effort");
                    // A per-tool-call callback (the workflow `agent` macro passes one to
                    // journal each genuine tool call as an `agent.tool_call` event).
                    on_tool_call = opts.get(&Value::keyword("on-tool-call")).cloned();
                    if let Some(t) = opts.get(&Value::keyword("tools")).and_then(|v| v.as_seq()) {
                        tools = t.to_vec();
                    }
                    if let Some(mode) = opts.get(&Value::keyword("tool-mode")) {
                        if let Some(s) = mode.as_keyword() {
                            tool_mode = s;
                        }
                    }
                    if let Some(rounds) = opts.get(&Value::keyword("max-tool-rounds")) {
                        if let Some(n) = rounds.as_int() {
                            max_tool_rounds = n as usize;
                        }
                    }
                }
            }

            // Per-call observability tags/metadata for both the no-tools (do_complete)
            // and tool-loop (run_tool_loop) branches below. Bound here so the guard
            // outlives the dispatch.
            let _tele = install_call_telemetry(args.get(1).and_then(|v| v.as_map_rc()).as_ref());

            if tools.is_empty() || tool_mode == "none" {
                // Simple chat without tools
                let mut request = ChatRequest::new(model, messages);
                request.max_tokens = max_tokens.or(Some(4096));
                request.temperature = temperature;
                request.system = system;
                request.reasoning_effort = reasoning_effort;
                request.timeout_ms = opt_timeout_ms(args.get(1));
                let _conv = conv_scope.open();

                // Native provider work in root-main and spawned tasks suspends on
                // an External wait; only a host call uses the synchronous adapter.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    dispatch_complete_offload(
                        request,
                        CompleteFinalize::new(|resp| Ok(Value::string(&resp.content))),
                    )
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let response = do_complete(request)?;
                    track_usage(&response.usage)?;
                    Ok(NativeOutcome::Return(Value::string(&response.content)))
                }
            } else {
                if sema_core::in_runtime_quantum() {
                    return Err(SemaError::eval(
                        "__llm-chat-blocking cannot run a tool loop inside the cooperative runtime",
                    )
                    .with_hint("call llm/chat so provider, observer, and tool callbacks can suspend cooperatively"));
                }
                // Chat with tool execution loop, synchronously — `run_tool_loop` is a
                // Rust `for` over rounds that blocks the VM thread per round. Reached
                // from top level (this IS "the" tool loop there) and, in principle,
                // from async context too, but the prelude dispatcher never routes an
                // async call with a configured tool loop here: `__chat-begin` returns
                // a token (not nil) whenever `tools`/`tool-mode` would take this
                // branch, so the dispatcher drives `__agent-step`/`__agent-exec-tools`
                // instead (see the `llm/chat` prelude entry, sema-eval/prelude.rs).
                let tool_schemas = build_tool_schemas(&tools)?;
                let (result, _msgs) = run_tool_loop(
                    ctx,
                    messages,
                    model,
                    max_tokens,
                    temperature,
                    system,
                    reasoning_effort,
                    &tools,
                    &tool_schemas,
                    max_tool_rounds,
                    on_tool_call.as_ref(),
                    None, // on_text: llm/chat doesn't stream
                    None, // agent_name
                    conv_scope,
                )?;
                Ok(NativeOutcome::Return(Value::string(&result)))
            }
        },
    );

    // (llm/send prompt {:model "..." ...})
    register_runtime_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/send", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/send", "1-2", args.len()));
        }
        let prompt = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        complete_with_prompt(&prompt, args.get(1))
    });

    // (llm/set-default :provider-name) — switch the active provider
    register_fn(env, "llm/set-default", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/set-default", "1", args.len()));
        }
        let name = args[0]
            .as_keyword()
            .or_else(|| args[0].as_str().map(|s| s.to_string()))
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            if reg.get(&name).is_some() {
                reg.set_default(&name);
                Ok(Value::keyword(&name))
            } else {
                Err(SemaError::Llm(format!("provider not configured: {name}")))
            }
        })
    });

    // (llm/list-providers) — list configured providers
    register_fn(env, "llm/list-providers", |_args| {
        PROVIDER_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            let names: Vec<Value> = reg
                .provider_names()
                .into_iter()
                .map(|n| Value::keyword(&n))
                .collect();
            Ok(Value::list(names))
        })
    });

    // (llm/current-provider) — query active provider/model
    register_fn(env, "llm/current-provider", |_args| {
        PROVIDER_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            match reg.default_provider() {
                Some(p) => {
                    let mut map = BTreeMap::new();
                    map.insert(Value::keyword("name"), Value::keyword(p.name()));
                    map.insert(Value::keyword("model"), Value::string(p.default_model()));
                    Ok(Value::map(map))
                }
                None => Ok(Value::nil()),
            }
        })
    });

    // (llm/pricing-status)
    register_fn(env, "llm/pricing-status", |_args| {
        let (source, updated_at) = pricing::pricing_status();
        let mut map = std::collections::BTreeMap::new();
        map.insert(Value::keyword("source"), Value::symbol(source));
        if let Some(date) = updated_at {
            map.insert(Value::keyword("updated-at"), Value::string(&date));
        }
        Ok(Value::map(map))
    });

    register_fn(env, "llm/providers", |_args| {
        let names = PROVIDER_REGISTRY.with(|reg| reg.borrow().provider_names());
        Ok(Value::list(
            names.into_iter().map(|n| Value::keyword(&n)).collect(),
        ))
    });

    register_fn(env, "llm/default-provider", |_args| {
        let name = PROVIDER_REGISTRY.with(|reg| {
            reg.borrow()
                .default_provider()
                .map(|p| p.name().to_string())
        });
        match name {
            Some(n) => Ok(Value::keyword(&n)),
            None => Ok(Value::nil()),
        }
    });

    // --- Token counting builtins ---

    register_fn(env, "llm/token-count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/token-count", "1", args.len()));
        }
        let char_count = if let Some(s) = args[0].as_str() {
            s.len()
        } else if let Some(list) = args[0].as_seq() {
            list.iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.len())
                        .unwrap_or_else(|| v.to_string().len())
                })
                .sum()
        } else {
            args[0].to_string().len()
        };
        Ok(Value::int((char_count / 4) as i64))
    });

    register_fn(env, "llm/token-estimate", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/token-estimate", "1", args.len()));
        }
        let char_count = if let Some(s) = args[0].as_str() {
            s.len()
        } else {
            args[0].to_string().len()
        };
        let tokens = (char_count / 4) as i64;
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("tokens"), Value::int(tokens));
        map.insert(Value::keyword("method"), Value::string("chars/4"));
        map.insert(Value::keyword("chars"), Value::int(char_count as i64));
        Ok(Value::map(map))
    });

    // --- Vector store builtins ---

    register_runtime_fn_ctx(env, "llm/summarize", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/summarize", "1-2", args.len()));
        }
        let text = args.str_at(0, "llm/summarize")?;

        let mut model = String::new();
        let mut max_length: Option<u32> = None;
        let mut style = "paragraph".to_string();

        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            model = opts.opt_str("model").unwrap_or_default();
            max_length = opts.opt_int("max-length").map(|n| n as u32);
            if let Some(s) = opts.opt_str("style") {
                style = s;
            }
        }

        let style_instruction = match style.as_str() {
            "bullet-points" | "bullets" => "Use bullet points.",
            "one-line" => "Respond with a single sentence summary.",
            _ => "Write a concise paragraph summary.",
        };
        let length_instruction = match max_length {
            Some(n) => format!(" Keep the summary under {} words.", n),
            None => String::new(),
        };
        let system =
            format!("Summarize the following text. {style_instruction}{length_instruction}");

        let messages = vec![ChatMessage::new("user", text)];
        let mut request = ChatRequest::new(model, messages);
        request.system = Some(system);
        request.max_tokens = Some(4096);

        // Runtime roots and spawned tasks suspend on an External wait; only a
        // host call uses the synchronous adapter.
        #[cfg(not(target_arch = "wasm32"))]
        {
            dispatch_complete_offload(
                request,
                CompleteFinalize::new(|resp| Ok(Value::string(&resp.content))),
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let response = do_complete(request)?;
            track_usage(&response.usage)?;
            Ok(NativeOutcome::Return(Value::string(&response.content)))
        }
    });

    register_runtime_fn_ctx(env, "llm/compare", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/compare", "2-3", args.len()));
        }
        let text_a = args.str_at(0, "llm/compare")?;
        let text_b = args.str_at(1, "llm/compare")?;

        let mut model = String::new();
        if let Some(opts) = args.get(2).and_then(|v| v.as_map_rc()) {
            model = opts.opt_str("model").unwrap_or_default();
        }

        let system =
            "Compare the following two texts. Respond with ONLY a JSON object containing:\n\
            - \"similarity\": a number from 0.0 (completely different) to 1.0 (identical)\n\
            - \"differences\": a list of key differences\n\
            - \"summary\": a brief comparison summary\n\
            Do not include any other text."
                .to_string();

        let user_msg = format!("Text A:\n{text_a}\n\nText B:\n{text_b}");
        let messages = vec![ChatMessage::new("user", &user_msg)];
        let mut request = ChatRequest::new(model, messages);
        request.system = Some(system);

        // Parse the comparison JSON out of the reply. Shared by the sync and
        // async paths.
        let parse_comparison = |response: ChatResponse| -> Result<Value, SemaError> {
            let content = response.content.trim();
            let json_str = if content.starts_with("```") {
                content
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim()
            } else {
                content
            };
            let json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
                SemaError::Llm(format!(
                    "failed to parse comparison JSON: {e}\nResponse: {content}"
                ))
            })?;
            Ok(sema_core::json_to_value(&json))
        };

        // Runtime roots and spawned tasks suspend on an External wait; only a
        // host call uses the synchronous adapter.
        #[cfg(not(target_arch = "wasm32"))]
        {
            dispatch_complete_offload(request, CompleteFinalize::new(parse_comparison))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let response = do_complete(request)?;
            track_usage(&response.usage)?;
            parse_comparison(response).map(NativeOutcome::Return)
        }
    });
}
