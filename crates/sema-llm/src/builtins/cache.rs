use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct CachedResponse {
    pub(super) content: String,
    /// Provider that served this response. An empty value marks a legacy cache
    /// entry, which an active policy rejects.
    #[serde(default)]
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) prompt_tokens: u32,
    pub(super) completion_tokens: u32,
    pub(super) cached_at: i64,
    /// Assistant tool calls retained for agent-loop cache replay.
    #[serde(default)]
    pub(super) tool_calls: Vec<ToolCall>,
}

pub(super) fn compute_cache_key(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.model.as_bytes());
    if let Some(temp) = request.temperature {
        hasher.update(temp.to_le_bytes());
    }
    if let Some(ref system) = request.system {
        hasher.update(system.as_bytes());
    }
    for msg in &request.messages {
        hasher.update(msg.role.as_bytes());
        hasher.update(msg.content.to_text().as_bytes());
    }
    // `max_tokens` and the tool schemas are part of what was ASKED, so they
    // belong in the key. Without them `(llm/complete p {:max-tokens 100})`
    // poisoned the entry for a later `{:max-tokens 4000}` call, and a
    // tool-bearing agent round shared a key with a bare completion of the same
    // prompt.
    if let Some(max_tokens) = request.max_tokens {
        hasher.update(b"\x00max_tokens\x00");
        hasher.update(max_tokens.to_le_bytes());
    }
    for tool in &request.tools {
        hasher.update(b"\x00tool\x00");
        hasher.update(tool.name.as_bytes());
        if let Ok(schema) = serde_json::to_string(&tool.parameters) {
            hasher.update(schema.as_bytes());
        }
    }
    let policy_fingerprint = effective_policy_fingerprint();
    if !policy_fingerprint.is_empty() {
        hasher.update(b"\x00policy\x00");
        hasher.update(policy_fingerprint.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(super) fn cache_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".sema")
        .join("cache")
        .join("llm")
}

pub(super) fn cache_file_path(key: &str) -> std::path::PathBuf {
    cache_dir().join(format!("{key}.json"))
}

pub(super) fn load_cached(key: &str) -> Option<CachedResponse> {
    if let Some(cached) = load_cached_mem(key) {
        return Some(cached);
    }
    let cached = read_cached_from_disk(&cache_file_path(key))?;
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    Some(cached)
}

/// In-memory (per-task-scope) cache probe only — never touches disk. The VM-thread
/// preparation stage uses this so a mem hit short-circuits while the disk read is
/// offloaded onto the runtime's blocking tier (the cache-peek phase below).
pub(super) fn load_cached_mem(key: &str) -> Option<CachedResponse> {
    CACHE_MEM.with(|c| c.borrow().get(key).cloned())
}

/// Read one cache entry off DISK. The single cache-read filesystem site; MUST run OFF
/// the runtime quantum — on the blocking tier (the driver's cache-peek phase) or the
/// host thread. A missing/corrupt file is a miss (`None`), never an error.
pub(super) fn read_cached_from_disk(path: &std::path::Path) -> Option<CachedResponse> {
    note_off_quantum_fs("llm cache read");
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub(super) fn store_cached(key: &str, response: &ChatResponse, provider: &str) {
    let cached = CachedResponse {
        content: response.content.clone(),
        provider: provider.to_string(),
        model: response.model.clone(),
        prompt_tokens: response.usage.prompt_tokens,
        completion_tokens: response.usage.completion_tokens,
        cached_at: unix_timestamp(),
        tool_calls: response.tool_calls.clone(),
    };
    // In-memory cache is authoritative for in-process reuse; update it on the VM
    // thread. Render the JSON here (bounded — a single response) and push the DISK
    // write off the quantum.
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    if let Ok(json) = serde_json::to_string(&cached) {
        persist_cache_file_off_quantum(cache_file_path(key), json);
    }
}

/// Persist a cache entry OFF the runtime quantum: on a blocking-tier worker when a
/// quantum is active, or synchronously on the host thread otherwise. Best-effort —
/// the in-memory cache already covers in-process reuse, so a dropped disk write only
/// forgoes cross-process persistence (the entry's existing trust model).
pub(super) fn persist_cache_file_off_quantum(path: std::path::PathBuf, json: String) {
    let write = move || {
        note_off_quantum_fs("llm cache write");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, json);
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        if sema_core::in_runtime_quantum() {
            sema_io::io_spawn_blocking(write);
            return;
        }
    }
    write();
}

/// Build the ZERO-usage `ChatResponse` served by a cache hit. A hit made NO provider
/// call, so it reports zero tokens: `track_usage` must not recharge session cost or
/// burn the budget for a cached response (the disk hit is identical to the mem hit).
pub(super) fn cache_hit_response(cached: CachedResponse, usage_model: String) -> ChatResponse {
    ChatResponse {
        content: cached.content,
        role: "assistant".to_string(),
        model: cached.model,
        tool_calls: cached.tool_calls,
        usage: Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            model: usage_model,
            ..Default::default()
        },
        stop_reason: Some("cache_hit".to_string()),
    }
}

pub(super) fn is_cache_valid(cached: &CachedResponse) -> bool {
    let ttl = CACHE_TTL_SECS.with(|c| c.get());
    (unix_timestamp() - cached.cached_at) < ttl
}

/// Resolve the model id used for the cache key when the caller pinned none. With an
/// active fallback chain, the "logical" model is the first chain entry's model (its
/// override if present, else that provider's default); otherwise it's the default
/// provider's default model.
pub(super) fn primary_model_for_cache() -> Result<String, SemaError> {
    let first_entry =
        FALLBACK_CHAIN.with(|c| c.borrow().as_ref().and_then(|chain| chain.first().cloned()));
    if let Some(entry) = first_entry {
        if let Some(model) = entry.model {
            return Ok(model);
        }
        return PROVIDER_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            reg.get(&entry.provider)
                .map(|p| p.default_model().to_string())
                .ok_or_else(|| {
                    SemaError::Llm(format!("fallback provider '{}' not found", entry.provider))
                })
        });
    }
    with_provider(|p| Ok(p.default_model().to_string()))
}

pub(super) fn register(env: &Env) {
    register_fn(env, "llm/cache-key", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/cache-key", "1-2", args.len()));
        }
        let prompt = args.str_at(0, "llm/cache-key")?;
        let mut model = String::new();
        let mut temperature = None;
        let mut system = None;
        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            model = opts.opt_str("model").unwrap_or_default();
            temperature = opts.opt_f64("temperature");
            system = opts.opt_str("system");
        }
        let messages = vec![ChatMessage::new("user", prompt)];
        let mut request = ChatRequest::new(model, messages);
        request.temperature = temperature;
        request.system = system;
        Ok(Value::string(&compute_cache_key(&request)))
    });

    register_fn(env, "llm/cache-clear", |_args| {
        let mem_count = CACHE_MEM.with(|c| {
            let mut cache = c.borrow_mut();
            let count = cache.len();
            cache.clear();
            count
        });
        let dir = cache_dir();
        if dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry
                        .path()
                        .extension()
                        .map(|e| e == "json")
                        .unwrap_or(false)
                    {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
        CACHE_HITS.with(|c| c.set(0));
        CACHE_MISSES.with(|c| c.set(0));
        Ok(Value::int(mem_count as i64))
    });

    register_fn(env, "llm/cache-stats", |_args| {
        let hits = CACHE_HITS.with(|c| c.get());
        let misses = CACHE_MISSES.with(|c| c.get());
        let size = CACHE_MEM.with(|c| c.borrow().len());
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("hits"), Value::int(hits as i64));
        map.insert(Value::keyword("misses"), Value::int(misses as i64));
        map.insert(Value::keyword("size"), Value::int(size as i64));
        Ok(Value::map(map))
    });

    register_scope_fn_ctx(env, "llm/with-cache", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/with-cache", "1-2", args.len()));
        }
        let (body_fn, ttl) = if args.len() == 2 {
            let opts = args.map_at(0, "llm/with-cache")?;
            let ttl = opts.opt_int("ttl").map(|n| n as u32).unwrap_or(3600) as i64;
            (&args[1], ttl)
        } else {
            (&args[0], 3600i64)
        };
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let prev_enabled = CACHE_ENABLED.with(|c| c.get());
        let prev_ttl = CACHE_TTL_SECS.with(|c| c.get());
        CACHE_ENABLED.with(|c| c.set(true));
        CACHE_TTL_SECS.with(|c| c.set(ttl));
        Ok((
            body_fn.clone(),
            Box::new(move || {
                CACHE_ENABLED.with(|c| c.set(prev_enabled));
                CACHE_TTL_SECS.with(|c| c.set(prev_ttl));
            }),
        ))
    });

    // --- Cassette (record/replay) builtins ---
}
