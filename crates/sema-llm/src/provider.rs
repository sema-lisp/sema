use crate::types::{
    ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, LlmError, RerankRequest, RerankResponse,
};

/// The boxed future shape of [`LlmProvider::complete_future`]. `Send` because it
/// is awaited inside a future spawned onto the shared I/O pool and may migrate
/// between worker threads.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxCompletionFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>,
>;

/// The boxed future shape of [`LlmProvider::embed_future`].
#[cfg(not(target_arch = "wasm32"))]
pub type BoxEmbedFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<EmbedResponse, LlmError>> + Send + 'a>,
>;

/// The core LLM provider trait. Sync interface — async internals are hidden —
/// plus optional future-returning hooks (`complete_future` / `embed_future`)
/// the async offload path uses to make cancellation REAL: an aborted spawned
/// provider future is dropped mid-flight (connection torn down) instead of
/// running to completion on a blocking worker.
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;
    fn default_model(&self) -> &str;

    /// Async completion hook for the cancellable offload path.
    ///
    /// `Some(fut)`: the provider exposes its native async implementation; the
    /// caller awaits it inside a future spawned on the shared I/O pool, so
    /// aborting the task drops the in-flight request with it (true
    /// cancellation). The future must be behaviorally identical to
    /// `complete()` — including any compat self-heal `complete()` performs.
    ///
    /// `None` (the default): the provider is sync-only; the caller offloads the
    /// blocking `complete()` to the pool's blocking tier instead, where
    /// cancellation stays best-effort (the result is discarded but the call
    /// runs to completion on the worker).
    #[cfg(not(target_arch = "wasm32"))]
    fn complete_future(&self, _request: ChatRequest) -> Option<BoxCompletionFuture<'_>> {
        None
    }

    /// Async embedding hook — the embeddings counterpart of
    /// [`complete_future`](Self::complete_future), with the same
    /// `Some` = true-cancel / `None` = best-effort-blocking-fallback contract.
    #[cfg(not(target_arch = "wasm32"))]
    fn embed_future(&self, _request: EmbedRequest) -> Option<BoxEmbedFuture<'_>> {
        None
    }

    /// Streaming — calls on_chunk for each text delta, returns full response at end.
    fn stream_complete(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        let resp = self.complete(request)?;
        on_chunk(&resp.content)?;
        Ok(resp)
    }

    /// Batch — run multiple requests concurrently.
    fn batch_complete(&self, requests: Vec<ChatRequest>) -> Vec<Result<ChatResponse, LlmError>> {
        requests.into_iter().map(|r| self.complete(r)).collect()
    }

    /// Embeddings.
    fn embed(&self, _request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        Err(LlmError::Config(format!(
            "{} does not support embeddings",
            self.name()
        )))
    }

    /// Cross-encoder reranking of `documents` against a `query`.
    fn rerank(&self, _request: RerankRequest) -> Result<RerankResponse, LlmError> {
        Err(LlmError::Config(format!(
            "{} does not support reranking",
            self.name()
        )))
    }
}

/// Registry of providers by name, plus a separate embedding provider slot.
///
/// Providers are stored as `Arc<dyn LlmProvider>` (the trait is `Send + Sync`)
/// so the VM thread can clone an `Arc` out of the thread-local registry, release
/// the borrow, and move it into a `spawn_blocking` future on the shared runtime —
/// the offloaded worker then calls the provider's own `complete`/`embed` with no
/// re-implementation. An `Arc<dyn LlmProvider>` derefs to `&dyn LlmProvider`, so
/// all synchronous `with_*_provider` callers keep working unchanged.
pub struct ProviderRegistry {
    providers: std::collections::HashMap<String, std::sync::Arc<dyn LlmProvider>>,
    default: Option<String>,
    embedding_provider: Option<String>,
    rerank_provider: Option<String>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        ProviderRegistry {
            providers: std::collections::HashMap::new(),
            default: None,
            embedding_provider: None,
            rerank_provider: None,
        }
    }

    /// Register a provider. Takes a `Box<dyn LlmProvider>` (so existing
    /// `register(Box::new(p))` call sites unsize-coerce as before) and stores it
    /// as an `Arc` (`Arc<T>: From<Box<T>>` for unsized `T`) so callers can later
    /// clone a `Send + Sync` handle out of the registry.
    pub fn register(&mut self, provider: Box<dyn LlmProvider>) {
        let provider: std::sync::Arc<dyn LlmProvider> = std::sync::Arc::from(provider);
        let name = provider.name().to_string();
        if self.default.is_none() {
            self.default = Some(name.clone());
        }
        self.providers.insert(name, provider);
    }

    pub fn get(&self, name: &str) -> Option<std::sync::Arc<dyn LlmProvider>> {
        self.providers.get(name).cloned()
    }

    pub fn default_provider(&self) -> Option<std::sync::Arc<dyn LlmProvider>> {
        self.default
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .cloned()
    }

    pub fn set_default(&mut self, name: &str) {
        if self.providers.contains_key(name) {
            self.default = Some(name.to_string());
        }
    }

    pub fn set_embedding_provider(&mut self, name: &str) {
        self.embedding_provider = Some(name.to_string());
    }

    pub fn embedding_provider(&self) -> Option<std::sync::Arc<dyn LlmProvider>> {
        self.embedding_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .cloned()
    }

    pub fn set_rerank_provider(&mut self, name: &str) {
        self.rerank_provider = Some(name.to_string());
    }

    /// The default rerank provider (last rerank-capable provider registered).
    pub fn rerank_provider(&self) -> Option<std::sync::Arc<dyn LlmProvider>> {
        self.rerank_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .cloned()
    }

    pub fn provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
