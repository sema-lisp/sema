//! A scripted, in-process [`LlmProvider`] for deterministic, key-free testing of
//! the LLM and agentic paths (completion, chat, streaming, batch, embeddings, and
//! the agent tool loop).
//!
//! This is test infrastructure — it is `pub` so integration tests in other crates
//! (`crates/sema/tests`) can use it, but it performs no network I/O and returns
//! only what it was scripted to return. Register one as the default provider with
//! [`crate::builtins::register_test_provider`].
//!
//! It records every request it receives into a shared [`FakeRecorder`], so tests
//! can assert on the exact messages the runtime built — including the round-2
//! tool-result messages the agent loop sends back, which is how the OpenAI
//! tool-protocol breakage is caught.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::provider::LlmProvider;
use crate::types::{
    ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, LlmError, RerankRequest,
    RerankResponse, RerankResult, ToolCall, Usage,
};

/// One scripted interaction. The fake pops these in order as calls arrive.
pub enum FakeReply {
    /// A plain chat/completion response.
    Chat(ChatResponse),
    /// A streamed response: emit `chunks` to `on_chunk`, then return `response`.
    Stream {
        chunks: Vec<String>,
        response: ChatResponse,
    },
    /// A streamed response that FAILS mid-stream: emit `chunks` to `on_chunk`, then
    /// return `error` (models a provider dropping the connection partway through).
    StreamThenError {
        chunks: Vec<String>,
        error: LlmError,
    },
    /// An embedding response.
    Embed(EmbedResponse),
    /// A rerank response (scored results, highest-first).
    Rerank(RerankResponse),
    /// Inject an error (for resilience tests).
    Error(LlmError),
}

/// Shared record of everything the fake provider was asked to do. The test holds
/// an `Arc` to this (via [`FakeProvider::recorder`]) so it can inspect requests
/// after the run, even though the provider itself is moved into the registry.
#[derive(Default)]
pub struct FakeRecorder {
    requests: Mutex<Vec<ChatRequest>>,
    embeds: Mutex<Vec<EmbedRequest>>,
    reranks: Mutex<Vec<RerankRequest>>,
}

impl FakeRecorder {
    /// All chat/completion requests received, in order.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// All embedding requests received, in order.
    pub fn embeds(&self) -> Vec<EmbedRequest> {
        self.embeds.lock().unwrap().clone()
    }

    /// All rerank requests received, in order.
    pub fn reranks(&self) -> Vec<RerankRequest> {
        self.reranks.lock().unwrap().clone()
    }

    /// Number of chat/completion calls made.
    pub fn call_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// A scripted [`LlmProvider`]. Build with [`FakeProvider::builder`].
pub struct FakeProvider {
    name: String,
    default_model: String,
    script: Mutex<VecDeque<FakeReply>>,
    recorder: Arc<FakeRecorder>,
    /// Fixed wall-clock delay injected into `embed()` (and only `embed()`), so a
    /// test can prove two concurrent `llm/embed`s overlap on the cooperative
    /// scheduler (wall ≈ max, not sum). 0 = no delay (default).
    embed_delay_ms: u64,
    /// Fixed wall-clock delay injected into `complete()` (and only `complete()`),
    /// so a test can prove two concurrent `llm/complete`/`classify`/`extract`s
    /// overlap on the cooperative scheduler (wall ≈ max, not sum) — the chat
    /// counterpart of `embed_delay_ms`. 0 = no delay (default).
    chat_delay_ms: u64,
    /// Fixed wall-clock delay injected BETWEEN chunks in `stream_complete()`, so
    /// a test can prove sibling tasks interleave between a stream's deltas (the
    /// streaming counterpart of `chat_delay_ms`). On the non-blocking stream path
    /// the chunks are emitted from a pool worker, so a real thread sleep is what
    /// spaces the deltas out in wall time. 0 = no delay (default).
    stream_chunk_delay_ms: u64,
    /// When set, `complete()` ignores the scripted queue and echoes the request's
    /// last user-message text as the response content. This correlates each reply
    /// to its prompt deterministically regardless of which `spawn_blocking` worker
    /// finishes first, so a test can prove `async/pool-map` preserves INPUT order
    /// even when concurrent completions land out of order.
    echo: bool,
    /// When set, `complete()` drives a deterministic multi-round tool loop keyed
    /// entirely on the REQUEST content (not a shared queue): if the request holds
    /// fewer than `rounds` assistant messages, it returns a tool call (a fresh id
    /// per depth); otherwise it returns the final `text`. Because the decision is a
    /// pure function of each request's own history, N concurrent multi-round
    /// `agent/run`s stay deterministic no matter how their rounds interleave on the
    /// scheduler — the property a shared `script` queue cannot provide. This is the
    /// interleave-safe oracle for non-blocking `agent/run`.
    tool_loop: Option<ToolLoopSpec>,
}

/// A deterministic, request-keyed multi-round tool-loop script (see
/// [`FakeProvider::tool_loop`]). Every concurrent agent independently walks the
/// same `rounds` tool calls then a final reply, keyed on its own message depth.
#[derive(Clone)]
pub struct ToolLoopSpec {
    /// Number of tool-call rounds emitted before the final text reply.
    rounds: usize,
    /// Tool name to invoke each round.
    tool_name: String,
    /// JSON arguments passed to the tool each round.
    args: serde_json::Value,
    /// Final assistant text returned once `rounds` tool rounds are complete.
    text: String,
}

impl FakeProvider {
    /// Start building a fake provider named `name` (the name the registry keys it
    /// under and that cost tracking attributes usage to).
    pub fn builder(name: &str) -> FakeProviderBuilder {
        FakeProviderBuilder {
            name: name.to_string(),
            default_model: "fake-model".to_string(),
            script: VecDeque::new(),
            embed_delay_ms: 0,
            chat_delay_ms: 0,
            stream_chunk_delay_ms: 0,
            echo: false,
            tool_loop: None,
        }
    }

    /// Grab a handle to the shared recorder *before* moving the provider into the
    /// registry, so the test can inspect recorded requests afterwards.
    pub fn recorder(&self) -> Arc<FakeRecorder> {
        self.recorder.clone()
    }
}

/// Builder for [`FakeProvider`]. Each method appends one scripted reply.
pub struct FakeProviderBuilder {
    name: String,
    default_model: String,
    script: VecDeque<FakeReply>,
    embed_delay_ms: u64,
    chat_delay_ms: u64,
    stream_chunk_delay_ms: u64,
    echo: bool,
    tool_loop: Option<ToolLoopSpec>,
}

impl FakeProviderBuilder {
    /// Set the model name reported by the provider.
    pub fn model(mut self, model: &str) -> Self {
        self.default_model = model.to_string();
        self
    }

    /// Script a plain text reply (default small usage).
    pub fn reply(mut self, text: &str) -> Self {
        self.script
            .push_back(FakeReply::Chat(self.chat_text(text, None)));
        self
    }

    /// Script a text reply with explicit token usage (for cost-tracking tests).
    pub fn reply_with_usage(
        mut self,
        text: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> Self {
        self.script.push_back(FakeReply::Chat(
            self.chat_text(text, Some((prompt_tokens, completion_tokens))),
        ));
        self
    }

    /// Script a text reply that also reports prompt-cache token counts — mirrors
    /// providers that surface `cache_read_input_tokens` / `cache_creation_input_tokens`.
    pub fn reply_with_cache_usage(
        mut self,
        text: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
        cache_read_input_tokens: u32,
        cache_creation_input_tokens: u32,
    ) -> Self {
        let mut resp = self.chat_text(text, Some((prompt_tokens, completion_tokens)));
        resp.usage.cache_read_input_tokens = cache_read_input_tokens;
        resp.usage.cache_creation_input_tokens = cache_creation_input_tokens;
        self.script.push_back(FakeReply::Chat(resp));
        self
    }

    /// Script an assistant turn that emits a single tool call (empty text content,
    /// `tool_use` stop reason) — mirrors how OpenAI/Anthropic return tool calls.
    pub fn tool_call(mut self, id: &str, name: &str, arguments: serde_json::Value) -> Self {
        let resp = ChatResponse {
            content: String::new(),
            role: "assistant".to_string(),
            model: self.default_model.clone(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments,
                thought_signature: None,
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                model: self.default_model.clone(),
                ..Default::default()
            },
            stop_reason: Some("tool_use".to_string()),
        };
        self.script.push_back(FakeReply::Chat(resp));
        self
    }

    /// Script a streamed reply: `chunks` are delivered to `on_chunk`, then the
    /// concatenation is returned as the final response.
    /// Script a stream that emits `chunks` then fails with `error` (mid-stream failure).
    pub fn stream_then_error(mut self, chunks: &[&str], error: LlmError) -> Self {
        self.script.push_back(FakeReply::StreamThenError {
            chunks: chunks.iter().map(|c| c.to_string()).collect(),
            error,
        });
        self
    }

    pub fn stream(mut self, chunks: &[&str]) -> Self {
        let full: String = chunks.concat();
        let response = self.chat_text(&full, None);
        self.script.push_back(FakeReply::Stream {
            chunks: chunks.iter().map(|c| c.to_string()).collect(),
            response,
        });
        self
    }

    /// Script an embedding reply.
    pub fn embed(mut self, vectors: Vec<Vec<f64>>) -> Self {
        let model = self.default_model.clone();
        self.script.push_back(FakeReply::Embed(EmbedResponse {
            embeddings: vectors,
            model: model.clone(),
            usage: Usage {
                prompt_tokens: 1,
                completion_tokens: 0,
                model,
                ..Default::default()
            },
        }));
        self
    }

    /// Script an embedding reply with an explicit prompt-token count (so a test
    /// can assert each concurrent embed carries its OWN input-token total on its
    /// own span — the per-task isolation proof).
    pub fn embed_with_tokens(mut self, vectors: Vec<Vec<f64>>, prompt_tokens: u32) -> Self {
        let model = self.default_model.clone();
        self.script.push_back(FakeReply::Embed(EmbedResponse {
            embeddings: vectors,
            model: model.clone(),
            usage: Usage {
                prompt_tokens,
                completion_tokens: 0,
                model,
                ..Default::default()
            },
        }));
        self
    }

    /// Inject a fixed wall-clock delay into `embed()` (only `embed()`), so two
    /// concurrent `llm/embed`s offloaded onto the shared runtime visibly overlap.
    pub fn embed_delay(mut self, ms: u64) -> Self {
        self.embed_delay_ms = ms;
        self
    }

    /// Inject a fixed wall-clock delay into `complete()` (only `complete()`), so
    /// two concurrent `llm/complete`/`classify`/`extract`s offloaded onto the
    /// shared runtime visibly overlap (chat counterpart of [`embed_delay`]).
    ///
    /// [`embed_delay`]: Self::embed_delay
    pub fn chat_delay(mut self, ms: u64) -> Self {
        self.chat_delay_ms = ms;
        self
    }

    /// Inject a fixed wall-clock delay BETWEEN chunks in `stream_complete()`, so a
    /// test can prove a sibling task advances between a stream's deltas (the
    /// streaming counterpart of [`chat_delay`]).
    ///
    /// [`chat_delay`]: Self::chat_delay
    pub fn stream_chunk_delay(mut self, ms: u64) -> Self {
        self.stream_chunk_delay_ms = ms;
        self
    }

    /// Make `complete()` echo the request's last user-message text instead of
    /// popping the scripted queue. Lets a concurrency test prove input-order
    /// preservation (the reply is a function of the prompt, not arrival order).
    /// Pairs with [`chat_delay`] for the overlap proof.
    ///
    /// [`chat_delay`]: Self::chat_delay
    pub fn echo(mut self) -> Self {
        self.echo = true;
        self
    }

    /// Drive a deterministic, request-keyed multi-round tool loop: emit `rounds`
    /// tool calls (invoking `tool_name` with `args`) then the final reply `text`.
    /// The round is decided per-request by counting assistant messages already in
    /// the request, so N concurrent `agent/run`s stay deterministic no matter how
    /// their rounds interleave (unlike the shared `script` queue). See
    /// [`ToolLoopSpec`]. Ignores/overrides the scripted queue and `echo`.
    pub fn tool_loop(
        mut self,
        rounds: usize,
        tool_name: &str,
        args: serde_json::Value,
        text: &str,
    ) -> Self {
        self.tool_loop = Some(ToolLoopSpec {
            rounds,
            tool_name: tool_name.to_string(),
            args,
            text: text.to_string(),
        });
        self
    }

    /// Script a rerank reply: `results` is `(original_index, relevance_score)` pairs,
    /// already ordered highest-relevance-first.
    pub fn rerank(mut self, results: &[(usize, f64)]) -> Self {
        let model = self.default_model.clone();
        self.script.push_back(FakeReply::Rerank(RerankResponse {
            results: results
                .iter()
                .map(|&(index, score)| RerankResult { index, score })
                .collect(),
            model,
        }));
        self
    }

    /// Script an injected error (e.g. `LlmError::RateLimited`, `LlmError::Api`).
    pub fn error(mut self, err: LlmError) -> Self {
        self.script.push_back(FakeReply::Error(err));
        self
    }

    /// Finish building.
    pub fn build(self) -> FakeProvider {
        FakeProvider {
            name: self.name,
            default_model: self.default_model,
            script: Mutex::new(self.script),
            recorder: Arc::new(FakeRecorder::default()),
            embed_delay_ms: self.embed_delay_ms,
            chat_delay_ms: self.chat_delay_ms,
            stream_chunk_delay_ms: self.stream_chunk_delay_ms,
            echo: self.echo,
            tool_loop: self.tool_loop,
        }
    }

    fn chat_text(&self, text: &str, usage: Option<(u32, u32)>) -> ChatResponse {
        let (p, c) = usage.unwrap_or((10, 5));
        ChatResponse {
            content: text.to_string(),
            role: "assistant".to_string(),
            model: self.default_model.clone(),
            tool_calls: Vec::new(),
            usage: Usage {
                prompt_tokens: p,
                completion_tokens: c,
                model: self.default_model.clone(),
                ..Default::default()
            },
            stop_reason: Some("end_turn".to_string()),
        }
    }
}

impl FakeProvider {
    fn next(&self) -> Option<FakeReply> {
        self.script.lock().unwrap().pop_front()
    }

    /// Build a plain text chat response with default small usage (echo mode).
    fn chat_text(&self, text: &str) -> ChatResponse {
        ChatResponse {
            content: text.to_string(),
            role: "assistant".to_string(),
            model: self.default_model.clone(),
            tool_calls: Vec::new(),
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                model: self.default_model.clone(),
                ..Default::default()
            },
            stop_reason: Some("end_turn".to_string()),
        }
    }
}

impl LlmProvider for FakeProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        // Echo mode: reply with the last user-message text BEFORE recording moves
        // the request. Correlates reply↔prompt deterministically (order-independent).
        let echoed = if self.echo {
            request
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.content.to_text())
        } else {
            None
        };
        // Deterministic multi-round tool loop keyed on THIS request's own history,
        // so concurrent agents stay reproducible under any interleaving. Decided
        // before `push` moves the request.
        let tool_loop_reply = self.tool_loop.as_ref().map(|spec| {
            let assistant_count = request
                .messages
                .iter()
                .filter(|m| m.role == "assistant")
                .count();
            if assistant_count < spec.rounds {
                // Another tool round: fresh id per depth so results correlate.
                ChatResponse {
                    content: String::new(),
                    role: "assistant".to_string(),
                    model: self.default_model.clone(),
                    tool_calls: vec![ToolCall {
                        id: format!("call_{assistant_count}"),
                        name: spec.tool_name.clone(),
                        arguments: spec.args.clone(),
                        thought_signature: None,
                    }],
                    usage: Usage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        model: self.default_model.clone(),
                        ..Default::default()
                    },
                    stop_reason: Some("tool_use".to_string()),
                }
            } else {
                self.chat_text(&spec.text)
            }
        });
        self.recorder.requests.lock().unwrap().push(request);
        // Injected latency: on the async path this runs on a spawn_blocking
        // worker, so a real thread sleep is what lets two completions overlap.
        if self.chat_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.chat_delay_ms));
        }
        if let Some(resp) = tool_loop_reply {
            return Ok(resp);
        }
        if let Some(text) = echoed {
            return Ok(self.chat_text(&text));
        }
        match self.next() {
            Some(FakeReply::Chat(r)) => Ok(r),
            Some(FakeReply::Stream { response, .. }) => Ok(response),
            Some(FakeReply::StreamThenError { error, .. }) => Err(error),
            Some(FakeReply::Error(e)) => Err(e),
            Some(FakeReply::Embed(_)) | Some(FakeReply::Rerank(_)) => Err(LlmError::Config(
                "FakeProvider: complete() reached an embed/rerank-scripted reply".to_string(),
            )),
            None => Err(LlmError::Config(
                "FakeProvider: no scripted reply left for complete()".to_string(),
            )),
        }
    }

    fn stream_complete(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        self.recorder.requests.lock().unwrap().push(request);
        // Real thread sleep between chunks (never before the first): on the
        // non-blocking stream path this runs on a pool worker, so the sleep is
        // what gives a sibling scheduler task wall time between deltas.
        let paced = |i: usize| {
            if i > 0 && self.stream_chunk_delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(self.stream_chunk_delay_ms));
            }
        };
        match self.next() {
            Some(FakeReply::Stream { chunks, response }) => {
                for (i, c) in chunks.iter().enumerate() {
                    paced(i);
                    on_chunk(c)?;
                }
                Ok(response)
            }
            Some(FakeReply::StreamThenError { chunks, error }) => {
                // Deliver the partial chunks to the callback, THEN fail.
                for (i, c) in chunks.iter().enumerate() {
                    paced(i);
                    on_chunk(c)?;
                }
                Err(error)
            }
            Some(FakeReply::Chat(r)) => {
                on_chunk(&r.content)?;
                Ok(r)
            }
            Some(FakeReply::Error(e)) => Err(e),
            Some(FakeReply::Embed(_)) | Some(FakeReply::Rerank(_)) => Err(LlmError::Config(
                "FakeProvider: stream_complete() reached an embed/rerank-scripted reply"
                    .to_string(),
            )),
            None => Err(LlmError::Config(
                "FakeProvider: no scripted reply left for stream_complete()".to_string(),
            )),
        }
    }

    fn batch_complete(&self, requests: Vec<ChatRequest>) -> Vec<Result<ChatResponse, LlmError>> {
        requests.into_iter().map(|r| self.complete(r)).collect()
    }

    fn embed(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        self.recorder.embeds.lock().unwrap().push(request);
        // Injected latency: on the async path this runs on a spawn_blocking
        // worker, so a real thread sleep is what lets two embeds overlap.
        if self.embed_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.embed_delay_ms));
        }
        match self.next() {
            Some(FakeReply::Embed(r)) => Ok(r),
            Some(FakeReply::Error(e)) => Err(e),
            _ => Err(LlmError::Config(
                "FakeProvider: no scripted embed reply left".to_string(),
            )),
        }
    }

    fn rerank(&self, request: RerankRequest) -> Result<RerankResponse, LlmError> {
        self.recorder.reranks.lock().unwrap().push(request);
        match self.next() {
            Some(FakeReply::Rerank(r)) => Ok(r),
            Some(FakeReply::Error(e)) => Err(e),
            _ => Err(LlmError::Config(
                "FakeProvider: no scripted rerank reply left".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_replies_in_order_and_records_requests() {
        let fake = FakeProvider::builder("fake")
            .tool_call("call_1", "get_weather", serde_json::json!({"city": "Oslo"}))
            .reply("It is sunny")
            .build();
        let recorder = fake.recorder();

        let r1 = fake
            .complete(ChatRequest::new("fake-model".into(), vec![]))
            .unwrap();
        assert_eq!(r1.tool_calls.len(), 1);
        assert_eq!(r1.tool_calls[0].name, "get_weather");

        let r2 = fake
            .complete(ChatRequest::new("fake-model".into(), vec![]))
            .unwrap();
        assert_eq!(r2.content, "It is sunny");
        assert!(r2.tool_calls.is_empty());

        assert_eq!(recorder.call_count(), 2);
    }

    #[test]
    fn chat_delay_is_honored_by_complete() {
        let delay = 80u64;
        let fake = FakeProvider::builder("fake")
            .chat_delay(delay)
            .reply("hi")
            .build();
        let t0 = std::time::Instant::now();
        let r = fake
            .complete(ChatRequest::new("fake-model".into(), vec![]))
            .unwrap();
        let elapsed = t0.elapsed();
        assert_eq!(r.content, "hi");
        assert!(
            elapsed >= std::time::Duration::from_millis(delay),
            "complete() should block for at least the chat delay; got {elapsed:?}"
        );
    }

    #[test]
    fn no_chat_delay_by_default() {
        let fake = FakeProvider::builder("fake").reply("hi").build();
        let t0 = std::time::Instant::now();
        let _ = fake
            .complete(ChatRequest::new("fake-model".into(), vec![]))
            .unwrap();
        assert!(
            t0.elapsed() < std::time::Duration::from_millis(50),
            "complete() should not block when no chat delay is set"
        );
    }

    #[test]
    fn injects_errors() {
        let fake = FakeProvider::builder("fake")
            .error(LlmError::RateLimited { retry_after_ms: 1 })
            .build();
        let err = fake
            .complete(ChatRequest::new("fake-model".into(), vec![]))
            .unwrap_err();
        assert!(matches!(err, LlmError::RateLimited { .. }));
    }
}
