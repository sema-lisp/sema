use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use sema_core::{
    resolve, Conversation, Env, EvalContext, ImageAttachment, Message, NativeFn, Prompt, Role,
    SemaError, Value, ValueView,
};

use sha2::{Digest, Sha256};

use crate::anthropic::AnthropicProvider;
use crate::embeddings::{CohereEmbeddingProvider, OpenAiCompatEmbeddingProvider};
use crate::gemini::GeminiProvider;
use crate::ollama::OllamaProvider;
use crate::openai::OpenAiProvider;
use crate::pricing;
use crate::provider::{LlmProvider, ProviderRegistry};
use crate::types::{
    ChatMessage, ChatRequest, ChatResponse, ContentBlock, EmbedRequest, LlmError, ToolCall,
    ToolSchema, Usage,
};
use crate::vector_store::{VectorDocument, VectorStore};

/// Type for a full evaluator callback: (ctx, expr, env) -> Result<Value, SemaError>
pub type EvalCallback = Box<dyn Fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError>>;

thread_local! {
    static PROVIDER_REGISTRY: RefCell<ProviderRegistry> = RefCell::new(ProviderRegistry::new());
    static SESSION_USAGE: RefCell<Usage> = RefCell::new(Usage::default());
    static LAST_USAGE: RefCell<Option<Usage>> = const { RefCell::new(None) };
    static EVAL_FN: RefCell<Option<EvalCallback>> = RefCell::new(None);
    static SESSION_COST: RefCell<f64> = const { RefCell::new(0.0) };
    static BUDGET_LIMIT: RefCell<Option<f64>> = const { RefCell::new(None) };
    static BUDGET_SPENT: RefCell<f64> = const { RefCell::new(0.0) };
    static BUDGET_TOKEN_LIMIT: RefCell<Option<u64>> = const { RefCell::new(None) };
    static BUDGET_TOKENS_SPENT: RefCell<u64> = const { RefCell::new(0) };
    static BUDGET_STACK: RefCell<Vec<BudgetFrame>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct BudgetFrame {
    cost_limit: Option<f64>,
    cost_spent: f64,
    token_limit: Option<u64>,
    tokens_spent: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedResponse {
    content: String,
    model: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    cached_at: i64,
}

/// One entry in an `llm/with-fallback` chain: a provider name plus an optional
/// per-provider model override. When `model` is `Some`, that model id is used for
/// this provider regardless of any model pinned in the call body (chain override
/// wins) — this lets a chain target a different model per provider, e.g. Opus on
/// Anthropic but a GPT model on OpenAI. When `None`, the provider's configured
/// default model is used.
#[derive(Debug, Clone)]
struct FallbackEntry {
    provider: String,
    model: Option<String>,
}

thread_local! {
    static PRICING_WARNING_SHOWN: Cell<bool> = const { Cell::new(false) };
    static LISP_PROVIDERS: RefCell<std::collections::HashMap<String, LispProviderCallbacks>> = RefCell::new(std::collections::HashMap::new());
    static CACHE_ENABLED: Cell<bool> = const { Cell::new(false) };
    static CACHE_MEM: RefCell<std::collections::HashMap<String, CachedResponse>> =
        RefCell::new(std::collections::HashMap::new());
    static CACHE_TTL_SECS: Cell<i64> = const { Cell::new(3600) };
    static CACHE_HITS: Cell<u64> = const { Cell::new(0) };
    static CACHE_MISSES: Cell<u64> = const { Cell::new(0) };
    static FALLBACK_CHAIN: RefCell<Option<Vec<FallbackEntry>>> = const { RefCell::new(None) };
    static VECTOR_STORES: RefCell<std::collections::HashMap<String, VectorStore>> =
        RefCell::new(std::collections::HashMap::new());
    static RATE_LIMIT_RPS: Cell<Option<f64>> = const { Cell::new(None) };
    static RATE_LIMIT_LAST: Cell<u64> = const { Cell::new(0) };
    // Name of the provider that served the most recent `do_complete` response, so cost
    // tracking can price the model as served by that provider (resellers/gateways can list
    // the same model id at a different rate). Set at the dispatch choke points, consumed +
    // cleared by `track_usage`. `None` → canonical first-party price.
    static LAST_SERVING_PROVIDER: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_serving_provider(name: &str) {
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = Some(name.to_string()));
}

fn take_serving_provider() -> Option<String> {
    LAST_SERVING_PROVIDER.with(|p| p.borrow_mut().take())
}

struct LispProviderCallbacks {
    complete_fn: Value,
}

/// Register a full evaluator for use by tool handlers and other LLM builtins.
pub fn set_eval_callback(
    f: impl Fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError> + 'static,
) {
    EVAL_FN.with(|eval| {
        *eval.borrow_mut() = Some(Box::new(f));
    });
}

/// Reset LLM runtime state used by builtins.
/// Called by interpreter construction to avoid cross-instance leakage.
pub fn reset_runtime_state() {
    PROVIDER_REGISTRY.with(|r| *r.borrow_mut() = ProviderRegistry::new());
    SESSION_USAGE.with(|u| *u.borrow_mut() = Usage::default());
    LAST_USAGE.with(|u| *u.borrow_mut() = None);
    EVAL_FN.with(|e| *e.borrow_mut() = None);
    SESSION_COST.with(|c| *c.borrow_mut() = 0.0);
    BUDGET_LIMIT.with(|l| *l.borrow_mut() = None);
    BUDGET_SPENT.with(|s| *s.borrow_mut() = 0.0);
    BUDGET_TOKEN_LIMIT.with(|l| *l.borrow_mut() = None);
    BUDGET_TOKENS_SPENT.with(|s| *s.borrow_mut() = 0);
    BUDGET_STACK.with(|s| s.borrow_mut().clear());
    PRICING_WARNING_SHOWN.with(|shown| shown.set(false));
    LISP_PROVIDERS.with(|p| p.borrow_mut().clear());
    CACHE_ENABLED.with(|c| c.set(false));
    CACHE_MEM.with(|c| c.borrow_mut().clear());
    CACHE_TTL_SECS.with(|c| c.set(3600));
    CACHE_HITS.with(|c| c.set(0));
    CACHE_MISSES.with(|c| c.set(0));
    FALLBACK_CHAIN.with(|c| *c.borrow_mut() = None);
    VECTOR_STORES.with(|s| s.borrow_mut().clear());
    RATE_LIMIT_RPS.with(|r| r.set(None));
    RATE_LIMIT_LAST.with(|r| r.set(0));
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = None);
    RETRY_BASE_MS.with(|c| c.set(500));
    NETWORK_MAX_RETRIES.with(|c| c.set(3));
    pricing::clear_custom_pricing();
}

/// Test-only: register `provider` as the default LLM provider, bypassing
/// `llm/configure`. Lets integration tests drive the completion/agent paths with
/// a scripted [`crate::fake::FakeProvider`] — no API keys, fully deterministic.
/// Call [`reset_runtime_state`] first to clear any prior provider.
pub fn register_test_provider(provider: Box<dyn LlmProvider>) {
    let name = provider.name().to_string();
    PROVIDER_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        reg.register(provider);
        reg.set_default(&name);
    });
}

/// Evaluate an expression using the registered full evaluator.
fn full_eval(ctx: &EvalContext, expr: &Value, env: &Env) -> Result<Value, SemaError> {
    EVAL_FN.with(|eval_fn| {
        let eval_fn = eval_fn.borrow();
        match &*eval_fn {
            Some(f) => f(ctx, expr, env),
            None => simple_eval(ctx, expr, env),
        }
    })
}

fn register_fn(env: &Env, name: &str, f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::simple(name, f)),
    );
}

fn register_fn_ctx(
    env: &Env,
    name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, f)),
    );
}

fn with_provider<F, R>(f: F) -> Result<R, SemaError>
where
    F: FnOnce(&dyn LlmProvider) -> Result<R, SemaError>,
{
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.default_provider().ok_or_else(|| {
            SemaError::Llm(
                "no LLM provider configured. Use (llm/configure :anthropic {:api-key ...}) first"
                    .to_string(),
            )
        })?;
        f(provider)
    })
}

fn with_embedding_provider<F, R>(f: F) -> Result<R, SemaError>
where
    F: FnOnce(&dyn LlmProvider) -> Result<R, SemaError>,
{
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg
            .embedding_provider()
            .or_else(|| reg.default_provider())
            .ok_or_else(|| {
                SemaError::Llm(
                    "no embedding provider configured. Use (llm/configure-embeddings ...) first"
                        .to_string(),
                )
            })?;
        f(provider)
    })
}

fn track_usage(usage: &Usage) -> Result<(), SemaError> {
    // Price the model as served by the provider that produced this response (falls back to
    // the canonical first-party price when the serving provider is unknown).
    let provider = take_serving_provider().unwrap_or_default();
    let cost = pricing::calculate_cost_for(&provider, usage);
    let total_tokens = (usage.prompt_tokens + usage.completion_tokens) as u64;

    LAST_USAGE.with(|u| *u.borrow_mut() = Some(usage.clone()));
    SESSION_USAGE.with(|u| {
        let mut session = u.borrow_mut();
        session.prompt_tokens += usage.prompt_tokens;
        session.completion_tokens += usage.completion_tokens;
        session.cache_read_input_tokens += usage.cache_read_input_tokens;
        session.cache_creation_input_tokens += usage.cache_creation_input_tokens;
    });

    // Check token budget
    BUDGET_TOKEN_LIMIT.with(|limit| {
        let limit = limit.borrow();
        if let Some(max_tokens) = *limit {
            BUDGET_TOKENS_SPENT.with(|spent| {
                let mut spent = spent.borrow_mut();
                *spent += total_tokens;
                if *spent > max_tokens {
                    return Err(SemaError::Llm(format!(
                        "token budget exceeded: used {} of {} tokens",
                        *spent, max_tokens
                    )));
                }
                Ok(())
            })
        } else {
            Ok(())
        }
    })?;

    if let Some(c) = cost {
        SESSION_COST.with(|sc| *sc.borrow_mut() += c);

        // Check cost budget
        BUDGET_LIMIT.with(|limit| {
            let limit = limit.borrow();
            if let Some(max_cost) = *limit {
                BUDGET_SPENT.with(|spent| {
                    let mut spent = spent.borrow_mut();
                    *spent += c;
                    if *spent > max_cost {
                        return Err(SemaError::Llm(format!(
                            "budget exceeded: spent ${:.4} of ${:.4} limit",
                            *spent, max_cost
                        )));
                    }
                    Ok(())
                })
            } else {
                Ok(())
            }
        })?;
    } else {
        // Cost unknown — warn once if budget is active
        BUDGET_LIMIT.with(|limit| {
            if limit.borrow().is_some() {
                PRICING_WARNING_SHOWN.with(|shown| {
                    if !shown.get() {
                        shown.set(true);
                        eprintln!(
                            "Warning: pricing unknown for model '{}'; budget enforcement is best-effort",
                            usage.model
                        );
                    }
                });
            }
        });
    }

    Ok(())
}

/// Set a budget limit for LLM calls.
pub fn set_budget(max_cost_usd: f64) {
    BUDGET_LIMIT.with(|l| *l.borrow_mut() = Some(max_cost_usd));
    BUDGET_SPENT.with(|s| *s.borrow_mut() = 0.0);
}

/// Set a token budget limit for LLM calls.
pub fn set_token_budget(max_tokens: u64) {
    BUDGET_TOKEN_LIMIT.with(|l| *l.borrow_mut() = Some(max_tokens));
    BUDGET_TOKENS_SPENT.with(|s| *s.borrow_mut() = 0);
}

/// Clear the budget limit.
pub fn clear_budget() {
    BUDGET_LIMIT.with(|l| *l.borrow_mut() = None);
    BUDGET_TOKEN_LIMIT.with(|l| *l.borrow_mut() = None);
}

/// Push a scoped budget and reset spent for the new scope.
pub fn push_budget_scope(max_cost_usd: Option<f64>, max_tokens: Option<u64>) {
    let frame = BudgetFrame {
        cost_limit: BUDGET_LIMIT.with(|l| *l.borrow()),
        cost_spent: BUDGET_SPENT.with(|s| *s.borrow()),
        token_limit: BUDGET_TOKEN_LIMIT.with(|l| *l.borrow()),
        tokens_spent: BUDGET_TOKENS_SPENT.with(|s| *s.borrow()),
    };
    BUDGET_STACK.with(|stack| stack.borrow_mut().push(frame));
    if let Some(cost) = max_cost_usd {
        set_budget(cost);
    } else {
        BUDGET_LIMIT.with(|l| *l.borrow_mut() = None);
        BUDGET_SPENT.with(|s| *s.borrow_mut() = 0.0);
    }
    if let Some(tokens) = max_tokens {
        set_token_budget(tokens);
    } else {
        BUDGET_TOKEN_LIMIT.with(|l| *l.borrow_mut() = None);
        BUDGET_TOKENS_SPENT.with(|s| *s.borrow_mut() = 0);
    }
}

/// Pop a scoped budget and restore the previous budget state.
pub fn pop_budget_scope() {
    let prev = BUDGET_STACK.with(|stack| stack.borrow_mut().pop());
    if let Some(frame) = prev {
        BUDGET_LIMIT.with(|l| *l.borrow_mut() = frame.cost_limit);
        BUDGET_SPENT.with(|s| *s.borrow_mut() = frame.cost_spent);
        BUDGET_TOKEN_LIMIT.with(|l| *l.borrow_mut() = frame.token_limit);
        BUDGET_TOKENS_SPENT.with(|s| *s.borrow_mut() = frame.tokens_spent);
    } else {
        clear_budget();
        BUDGET_SPENT.with(|s| *s.borrow_mut() = 0.0);
        BUDGET_TOKENS_SPENT.with(|s| *s.borrow_mut() = 0);
    }
}

fn get_opt_string(opts: &BTreeMap<Value, Value>, key: &str) -> Option<String> {
    opts.get(&Value::keyword(key))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

/// Read an option that may be given as a keyword (`:high`) or a string
/// (`"high"`) — used for `:reasoning-effort`.
fn get_opt_effort(opts: &BTreeMap<Value, Value>, key: &str) -> Option<String> {
    opts.get(&Value::keyword(key))
        .and_then(|v| v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string())))
}

/// Parse one `llm/with-fallback` chain element into a [`FallbackEntry`].
///
/// Accepted shapes:
/// - `:provider` / `"provider"` — bare name, uses the provider's default model
/// - `[:provider "model"]` — pair, with a per-provider model override
/// - `{:provider :name :model "model"}` — map form, `:model` optional
fn parse_fallback_entry(v: &Value) -> Result<FallbackEntry, SemaError> {
    // Bare keyword or string.
    if let Some(name) = v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string())) {
        return Ok(FallbackEntry {
            provider: name,
            model: None,
        });
    }
    // Map form: {:provider .. :model ..}. The :provider value may be a keyword or
    // a string.
    if let Some(map) = v.as_map_ref() {
        let provider = map
            .get(&Value::keyword("provider"))
            .and_then(|p| p.as_keyword().or_else(|| p.as_str().map(|s| s.to_string())))
            .ok_or_else(|| {
                SemaError::eval("fallback map entry must have a :provider key (keyword or string)")
            })?;
        return Ok(FallbackEntry {
            provider,
            model: get_opt_string(map, "model"),
        });
    }
    // Pair form: [:provider "model"].
    if let Some(seq) = v.as_seq() {
        if seq.len() != 2 {
            return Err(SemaError::eval(
                "fallback pair entry must be [provider model]",
            ));
        }
        let provider = seq[0]
            .as_keyword()
            .or_else(|| seq[0].as_str().map(|s| s.to_string()))
            .ok_or_else(|| SemaError::type_error("keyword or string", seq[0].type_name()))?;
        let model = seq[1]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| SemaError::type_error("string model", seq[1].type_name()))?;
        return Ok(FallbackEntry {
            provider,
            model: Some(model),
        });
    }
    Err(SemaError::type_error(
        "keyword, string, [provider model] pair, or map",
        v.type_name(),
    ))
}

fn get_opt_f64(opts: &BTreeMap<Value, Value>, key: &str) -> Option<f64> {
    opts.get(&Value::keyword(key)).and_then(|v| v.as_float())
}

fn get_opt_u32(opts: &BTreeMap<Value, Value>, key: &str) -> Option<u32> {
    opts.get(&Value::keyword(key))
        .and_then(|v| v.as_int())
        .map(|n| n as u32)
}

/// Substitute `{{key}}` placeholders in a template string using a vars map.
/// Keys are looked up as keywords in the map. Unfilled slots are left as-is.
fn fill_template(template: &str, vars: &BTreeMap<Value, Value>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'{') {
            chars.next();
            let mut var_name = String::new();
            let mut found_close = false;
            while let Some(c) = chars.next() {
                if c == '}' && chars.peek() == Some(&'}') {
                    chars.next();
                    found_close = true;
                    break;
                }
                var_name.push(c);
            }
            if found_close {
                if let Some(val) = vars.get(&Value::keyword(&var_name)) {
                    if let Some(s) = val.as_str() {
                        result.push_str(s);
                    } else {
                        result.push_str(&val.to_string());
                    }
                } else {
                    result.push_str("{{");
                    result.push_str(&var_name);
                    result.push_str("}}");
                }
            } else {
                result.push_str("{{");
                result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// A provider defined in Sema code via lambdas.
/// Only stores String fields (Send+Sync); callbacks live in the
/// LISP_PROVIDERS thread-local, accessed only from the same thread.
struct LispProvider {
    name: String,
    default_model: String,
}

impl LlmProvider for LispProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let name = self.name.clone();
        LISP_PROVIDERS.with(|providers| {
            let providers = providers.borrow();
            let callbacks = providers.get(&name).ok_or_else(|| {
                LlmError::Config(format!("lisp provider '{}' callbacks not found", name))
            })?;
            let complete_fn = callbacks.complete_fn.clone();

            let request_map = chat_request_to_value(&request);

            let ctx = EvalContext::new();
            ctx.eval_step_limit.set(1_000_000);
            let result = call_value_fn(&ctx, &complete_fn, &[request_map]);

            match result {
                Ok(response_val) => parse_lisp_provider_response(&response_val, &request.model),
                Err(e) => Err(LlmError::Api {
                    status: 0,
                    message: e.to_string(),
                }),
            }
        })
    }
}

/// Convert a ChatRequest into a Sema Value::Map for passing to Lisp provider callbacks.
fn chat_request_to_value(request: &ChatRequest) -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("model"), Value::string(&request.model));

    let msgs: Vec<Value> = request
        .messages
        .iter()
        .map(|m| {
            let mut msg_map = BTreeMap::new();
            msg_map.insert(Value::keyword("role"), Value::string(&m.role));
            msg_map.insert(
                Value::keyword("content"),
                Value::string(&m.content.to_text()),
            );
            Value::map(msg_map)
        })
        .collect();
    map.insert(Value::keyword("messages"), Value::list(msgs));

    if let Some(max_tokens) = request.max_tokens {
        map.insert(Value::keyword("max-tokens"), Value::int(max_tokens as i64));
    }
    if let Some(temp) = request.temperature {
        map.insert(Value::keyword("temperature"), Value::float(temp));
    }
    if let Some(ref system) = request.system {
        map.insert(Value::keyword("system"), Value::string(system));
    }

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                let mut tool_map = BTreeMap::new();
                tool_map.insert(Value::keyword("name"), Value::string(&t.name));
                tool_map.insert(Value::keyword("description"), Value::string(&t.description));
                tool_map.insert(
                    Value::keyword("parameters"),
                    sema_core::json_to_value(&t.parameters),
                );
                Value::map(tool_map)
            })
            .collect();
        map.insert(Value::keyword("tools"), Value::list(tools));
    }

    if !request.stop_sequences.is_empty() {
        let seqs: Vec<Value> = request
            .stop_sequences
            .iter()
            .map(|s| Value::string(s))
            .collect();
        map.insert(Value::keyword("stop-sequences"), Value::list(seqs));
    }

    Value::map(map)
}

/// Parse a Sema Value returned by a Lisp provider callback into a ChatResponse.
fn parse_lisp_provider_response(val: &Value, model: &str) -> Result<ChatResponse, LlmError> {
    match val.view() {
        ValueView::String(s) => Ok(ChatResponse {
            content: s.to_string(),
            role: "assistant".to_string(),
            model: model.to_string(),
            tool_calls: vec![],
            usage: Usage::default(),
            stop_reason: Some("end_turn".to_string()),
        }),
        ValueView::Map(map) => {
            let content = map
                .get(&Value::keyword("content"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let role = map
                .get(&Value::keyword("role"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "assistant".to_string());
            let resp_model = map
                .get(&Value::keyword("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| model.to_string());
            let stop_reason = map
                .get(&Value::keyword("stop-reason"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(Some("end_turn".to_string()));

            let usage = if let Some(usage_val) = map.get(&Value::keyword("usage")) {
                if let Some(usage_map) = usage_val.as_map_rc() {
                    let prompt_tokens = usage_map
                        .get(&Value::keyword("prompt-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let completion_tokens = usage_map
                        .get(&Value::keyword("completion-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let cache_read_input_tokens = usage_map
                        .get(&Value::keyword("cache-read-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let cache_creation_input_tokens = usage_map
                        .get(&Value::keyword("cache-creation-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    Usage {
                        prompt_tokens,
                        completion_tokens,
                        model: resp_model.clone(),
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    }
                } else {
                    Usage {
                        model: resp_model.clone(),
                        ..Default::default()
                    }
                }
            } else {
                Usage {
                    model: resp_model.clone(),
                    ..Default::default()
                }
            };

            let tool_calls = if let Some(tcs_val) = map.get(&Value::keyword("tool-calls")) {
                if let Some(tcs) = tcs_val.as_seq() {
                    tcs.iter()
                        .filter_map(|tc| {
                            let tc_map = tc.as_map_rc()?;
                            let id = tc_map
                                .get(&Value::keyword("id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            let name = tc_map
                                .get(&Value::keyword("name"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())?;
                            let arguments = tc_map
                                .get(&Value::keyword("arguments"))
                                .map(sema_core::value_to_json_lossy)
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                            Some(ToolCall {
                                id,
                                name,
                                arguments,
                            })
                        })
                        .collect()
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            Ok(ChatResponse {
                content,
                role,
                model: resp_model,
                tool_calls,
                usage,
                stop_reason,
            })
        }
        _ => Err(LlmError::Parse(
            "lisp provider must return a string or map with :content".to_string(),
        )),
    }
}

fn register_fn_gated(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        });
    }
}

fn register_fn_ctx_gated(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    name: &str,
    f: impl Fn(&sema_core::EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn_ctx(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn_ctx(env, name, move |ctx, args| {
            sandbox.check(cap, &fn_name)?;
            f(ctx, args)
        });
    }
}

/// Extract the host from a provider `base-url`/`host` string without pulling in
/// a URL-parsing dependency. Handles `scheme://`, userinfo, `[ipv6]`, and ports.
fn url_host(url: &str) -> Option<String> {
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
fn is_internal_host(host: &str) -> bool {
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
fn ipv4_is_internal(v4: std::net::Ipv4Addr) -> bool {
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
fn parse_loose_ipv4(host: &str) -> Option<std::net::Ipv4Addr> {
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
fn parse_uint_part(s: &str) -> Option<u32> {
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
fn guard_provider_url(unrestricted: bool, opts: &BTreeMap<Value, Value>) -> Result<(), SemaError> {
    if unrestricted {
        return Ok(());
    }
    let url = get_opt_string(opts, "base-url").or_else(|| get_opt_string(opts, "host"));
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

pub fn register_llm_builtins(env: &Env, sandbox: &sema_core::Sandbox) {
    let unrestricted = sandbox.is_unrestricted();
    // (llm/configure :anthropic {:api-key "..." :default-model "..."})
    // (llm/configure :openai {:api-key "..." :base-url "..." :default-model "..."})
    register_fn(env, "llm/configure", move |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/configure", "2", args.len()));
        }
        let provider_name = args[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
        let opts_rc = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let opts = opts_rc.as_ref().clone();

        guard_provider_url(unrestricted, &opts)?;

        let api_key = get_opt_string(&opts, "api-key");

        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            match provider_name.as_str() {
                "anthropic" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = AnthropicProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("anthropic");
                }
                "openai" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let base_url = get_opt_string(&opts, "base-url");
                    let model = get_opt_string(&opts, "default-model");
                    let provider = OpenAiProvider::new(api_key, base_url, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("openai");
                }
                "gemini" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = GeminiProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("gemini");
                }
                "groq" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "llama-3.3-70b-versatile".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
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
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "grok-4.3".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
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
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "mistral-large-latest".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
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
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "kimi-k2.6".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
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
                        get_opt_string(&opts, "host").or_else(|| get_opt_string(&opts, "base-url"));
                    let model = get_opt_string(&opts, "default-model");
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
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "jina-embeddings-v3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        api_key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                }
                "voyage" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "voyage-3-lite".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        api_key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("voyage");
                }
                "cohere" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = CohereEmbeddingProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("cohere");
                }
                other => {
                    // Treat unknown providers as OpenAI-compatible if base-url and api-key are provided
                    let api_key = api_key.clone().ok_or_else(|| {
                        SemaError::Llm(format!(
                            "unknown provider '{other}': provide :api-key and :base-url to register as OpenAI-compatible"
                        ))
                    })?;
                    let base_url = get_opt_string(&opts, "base-url").ok_or_else(|| {
                        SemaError::Llm(format!(
                            "unknown provider '{other}': provide :base-url to register as OpenAI-compatible"
                        ))
                    })?;
                    let model = get_opt_string(&opts, "default-model")
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
        let provider_name = args[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
        let opts_rc = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let opts = opts_rc.as_ref().clone();

        let complete_fn = opts
            .get(&Value::keyword("complete"))
            .cloned()
            .ok_or_else(|| SemaError::eval("llm/define-provider requires :complete function"))?;

        if complete_fn.as_lambda_rc().is_none() && complete_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", complete_fn.type_name()));
        }

        let default_model =
            get_opt_string(&opts, "default-model").unwrap_or_else(|| "default".to_string());

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
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
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
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    // Only set as embedding provider if not already set
                    if reg.embedding_provider().is_none() {
                        reg.set_embedding_provider("voyage");
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
    register_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/complete", |args| {
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

        if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                max_tokens = get_opt_u32(&opts, "max-tokens");
                temperature = get_opt_f64(&opts, "temperature");
                system = get_opt_string(&opts, "system");
                reasoning_effort = get_opt_effort(&opts, "reasoning-effort");
            }
        }

        let messages = vec![ChatMessage::new("user", prompt_text)];

        let mut request = ChatRequest::new(model, messages);
        request.max_tokens = max_tokens.or(Some(4096));
        request.temperature = temperature;
        request.system = system;
        request.reasoning_effort = reasoning_effort;

        let response = do_complete(request)?;
        track_usage(&response.usage)?;
        Ok(Value::string(&response.content))
    });

    // (llm/chat messages {:model "..." :tools [...] :tool-mode :auto ...})
    register_fn_ctx_gated(
        env,
        sandbox,
        sema_core::Caps::LLM,
        "llm/chat",
        |ctx, args| {
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

            if let Some(opts_val) = args.get(1) {
                if let Some(opts) = opts_val.as_map_rc() {
                    model = get_opt_string(&opts, "model").unwrap_or_default();
                    max_tokens = get_opt_u32(&opts, "max-tokens");
                    temperature = get_opt_f64(&opts, "temperature");
                    system = get_opt_string(&opts, "system");
                    reasoning_effort = get_opt_effort(&opts, "reasoning-effort");
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

            if tools.is_empty() || tool_mode == "none" {
                // Simple chat without tools
                let mut request = ChatRequest::new(model, messages);
                request.max_tokens = max_tokens.or(Some(4096));
                request.temperature = temperature;
                request.system = system;
                request.reasoning_effort = reasoning_effort;
                let response = do_complete(request)?;
                track_usage(&response.usage)?;
                Ok(Value::string(&response.content))
            } else {
                // Chat with tool execution loop
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
                    None,
                    None,
                )?;
                Ok(Value::string(&result))
            }
        },
    );

    // (llm/send prompt {:model "..." ...})
    register_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/send", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/send", "1-2", args.len()));
        }
        let prompt = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        complete_with_prompt(&prompt, args.get(1))
    });

    // (llm/stream "prompt" callback {:max-tokens 200})
    // (llm/stream "prompt" {:max-tokens 200})  — prints to stdout
    register_fn_ctx(env, "llm/stream", |ctx, args| {
        if args.is_empty() || args.len() > 3 {
            return Err(SemaError::arity("llm/stream", "1-3", args.len()));
        }

        // Parse the prompt/messages
        let messages = if let Some(s) = args[0].as_str() {
            vec![ChatMessage::new("user", s)]
        } else if let Some(p) = args[0].as_prompt_rc() {
            p.messages
                .iter()
                .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
                .collect()
        } else if args[0].as_seq().is_some() {
            extract_messages(&args[0])?
        } else {
            return Err(SemaError::type_error(
                "string, prompt, or messages",
                args[0].type_name(),
            ));
        };

        // Parse optional callback and opts
        let mut callback: Option<Value> = None;
        let mut opts_map: Option<Rc<BTreeMap<Value, Value>>> = None;

        for arg in &args[1..] {
            if arg.as_lambda_rc().is_some() || arg.as_native_fn_rc().is_some() {
                callback = Some(arg.clone());
            } else if let Some(m) = arg.as_map_rc() {
                opts_map = Some(m);
            }
        }

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;

        if let Some(ref opts) = opts_map {
            model = get_opt_string(opts, "model").unwrap_or_default();
            max_tokens = get_opt_u32(opts, "max-tokens");
            temperature = get_opt_f64(opts, "temperature");
            system = get_opt_string(opts, "system");
        }

        let mut request = ChatRequest::new(model, messages);
        request.max_tokens = max_tokens.or(Some(4096));
        request.temperature = temperature;
        request.system = system;

        // Streaming bypasses do_complete/track_usage, so it gets its own CLIENT span.
        let span = sema_otel::llm_span("chat");
        span.set_request(
            request.temperature,
            request.max_tokens,
            &request.stop_sequences,
            None,
        );

        let response = with_provider(|p| {
            if request.model.is_empty() {
                let mut req = request.clone();
                req.model = p.default_model().to_string();
                let mut chunk_cb = |chunk: &str| -> Result<(), crate::types::LlmError> {
                    if let Some(ref cb) = callback {
                        call_value_fn(ctx, cb, &[Value::string(chunk)])
                            .map_err(|e| crate::types::LlmError::Config(e.to_string()))?;
                    } else {
                        print!("{}", chunk);
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    }
                    Ok(())
                };
                let req_model = req.model.clone();
                let resp = p
                    .stream_complete(req, &mut chunk_cb)
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                span.set_dispatch(p.name(), &req_model);
                span.set_response(&response_facts(p.name(), &resp));
                Ok(resp)
            } else {
                let mut chunk_cb = |chunk: &str| -> Result<(), crate::types::LlmError> {
                    if let Some(ref cb) = callback {
                        call_value_fn(ctx, cb, &[Value::string(chunk)])
                            .map_err(|e| crate::types::LlmError::Config(e.to_string()))?;
                    } else {
                        print!("{}", chunk);
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    }
                    Ok(())
                };
                let resp = p
                    .stream_complete(request.clone(), &mut chunk_cb)
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                span.set_dispatch(p.name(), &request.model);
                span.set_response(&response_facts(p.name(), &resp));
                Ok(resp)
            }
        })?;

        // Print newline after streaming if using default display
        if callback.is_none() {
            println!();
        }

        track_usage(&response.usage)?;
        Ok(Value::string(&response.content))
    });

    // (llm/extract schema text {:model "..." :validate true :retries 2 :reask? true})
    register_fn(env, "llm/extract", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/extract", "2-3", args.len()));
        }
        let schema = &args[0];
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let schema_desc = format_schema(schema);
        let system = format!(
            "Extract structured data from the text. Respond with ONLY a JSON object matching this schema:\n{}\nDo not include any other text.",
            schema_desc
        );
        let messages = vec![ChatMessage::new("user", text)];

        let mut model = String::new();
        let mut validate = true;
        let mut max_retries: u32 = 2;
        let mut reask = true;
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                if let Some(v) = opts.get(&Value::keyword("validate")) {
                    validate = v.is_truthy();
                }
                if let Some(r) = get_opt_u32(&opts, "retries") {
                    max_retries = r;
                }
                if let Some(v) = opts.get(&Value::keyword("reask?")) {
                    reask = v.is_truthy();
                }
            }
        }

        let mut last_validation_error = String::new();
        let mut last_response_content = String::new();

        for attempt in 0..=max_retries {
            let mut request = ChatRequest::new(model.clone(), messages.clone());
            request.json_mode = true;
            if attempt == 0 {
                request.system = Some(system.clone());
            } else if reask {
                request.system = Some(format_reask_prompt(
                    &last_response_content,
                    &last_validation_error,
                    &schema_desc,
                ));
            } else {
                request.system = Some(format!(
                    "{}\n\nYour previous response had validation errors: {}. Please fix.",
                    system, last_validation_error
                ));
            }

            let response = do_complete(request)?;
            track_usage(&response.usage)?;

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
                    "failed to parse LLM JSON response: {e}\nResponse was: {content}"
                ))
            })?;
            let result = sema_core::json_to_value(&json);

            if validate {
                match validate_extraction(&result, schema) {
                    Ok(()) => return Ok(result),
                    Err(err) => {
                        last_validation_error = err;
                        last_response_content = content.to_string();
                        if attempt == max_retries {
                            return Err(SemaError::Llm(format!(
                                "extraction validation failed after {} attempt(s): {}",
                                max_retries + 1,
                                last_validation_error
                            )));
                        }
                    }
                }
            } else {
                return Ok(result);
            }
        }

        unreachable!()
    });

    // (llm/extract-from-image schema source {:model "..."})
    // source: string path or bytevector
    register_fn_ctx_gated(
        env,
        sandbox,
        sema_core::Caps::LLM,
        "llm/extract-from-image",
        |_ctx, args| {
            if args.len() < 2 || args.len() > 3 {
                return Err(SemaError::arity(
                    "llm/extract-from-image",
                    "2-3",
                    args.len(),
                ));
            }
            let schema = &args[0];

            // Get image bytes: either from path (string) or bytevector
            let bytes = if let Some(path) = args[1].as_str() {
                std::fs::read(path)
                    .map_err(|e| SemaError::Io(format!("llm/extract-from-image: {path}: {e}")))?
            } else if let Some(bv) = args[1].as_bytevector() {
                bv.to_vec()
            } else {
                return Err(SemaError::type_error(
                    "string path or bytevector",
                    args[1].type_name(),
                ));
            };

            let media_type = detect_media_type(&bytes).to_string();
            use base64::Engine;
            let b64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

            let schema_desc = format_schema(schema);
            let system = format!(
                "Extract structured data from the image. Respond with ONLY a JSON object matching this schema:\n{}\nDo not include any other text.",
                schema_desc
            );

            let messages = vec![ChatMessage::with_blocks(
                "user",
                vec![
                    ContentBlock::Image {
                        media_type: Some(media_type),
                        data: b64_data,
                    },
                    ContentBlock::Text {
                        text: "Extract the requested data from this image. Respond in JSON."
                            .to_string(),
                    },
                ],
            )];

            let mut model = String::new();
            if let Some(opts_val) = args.get(2) {
                if let Some(opts) = opts_val.as_map_rc() {
                    model = get_opt_string(&opts, "model").unwrap_or_default();
                }
            }

            let mut request = ChatRequest::new(model, messages);
            request.system = Some(system);
            request.json_mode = true;

            let response = do_complete(request)?;
            track_usage(&response.usage)?;

            // Parse JSON response back to Sema value
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
                    "failed to parse LLM JSON response: {e}\nResponse was: {content}"
                ))
            })?;
            Ok(sema_core::json_to_value(&json))
        },
    );

    // (llm/classify categories text {:model "..."})
    register_fn(env, "llm/classify", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/classify", "2-3", args.len()));
        }
        let categories = args[0]
            .as_seq()
            .map(|l| l.to_vec())
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let cat_names: Vec<String> = categories
            .iter()
            .map(|c| {
                if let Some(kw) = c.as_keyword() {
                    kw
                } else if let Some(s) = c.as_str() {
                    s.to_string()
                } else {
                    c.to_string()
                }
            })
            .collect();

        let system = format!(
            "Classify the following text into exactly one of these categories: {}\nRespond with ONLY the category name, nothing else.",
            cat_names.join(", ")
        );
        let messages = vec![ChatMessage::new("user", text)];

        let mut model = String::new();
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
            }
        }

        let mut request = ChatRequest::new(model, messages);
        request.system = Some(system);

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

        let category = response.content.trim().to_string();
        // Return as keyword if it was in the original list as keyword
        if categories
            .iter()
            .any(|c| c.as_keyword().map(|kw| kw == category).unwrap_or(false))
        {
            Ok(Value::keyword(&category))
        } else {
            Ok(Value::string(&category))
        }
    });

    // Conversation functions

    // (conversation/new {:model "..."})
    register_fn(env, "conversation/new", |args| {
        let mut model = String::new();
        let mut metadata = BTreeMap::new();
        if let Some(opts_val) = args.first() {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                for (k, v) in opts.iter() {
                    if let Some(key_str) = k.as_keyword() {
                        if key_str != "model" {
                            metadata.insert(
                                key_str,
                                v.as_str()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| v.to_string()),
                            );
                        }
                    }
                }
            }
        }
        Ok(Value::conversation(Conversation {
            messages: Vec::new(),
            model,
            metadata,
        }))
    });

    // (conversation/say conv "message" {:temperature 0.5 :max-tokens 2048 :system "..."})
    register_fn(env, "conversation/say", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("conversation/say", "2-3", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let user_msg = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());

        // Parse optional opts
        let mut temperature = None;
        let mut max_tokens = None;
        let mut system = None;
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                temperature = get_opt_f64(&opts, "temperature");
                max_tokens = get_opt_u32(&opts, "max-tokens");
                system = get_opt_string(&opts, "system");
            }
        }

        // Build messages for API call
        let mut chat_messages: Vec<ChatMessage> = conv
            .messages
            .iter()
            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
            .collect();
        chat_messages.push(ChatMessage::new("user", user_msg.clone()));

        let mut request = ChatRequest::new(conv.model.clone(), chat_messages);
        request.temperature = temperature;
        request.max_tokens = max_tokens.or(Some(4096));
        request.system = system;

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

        // Build new conversation with user message + assistant reply
        let mut new_messages = conv.messages.clone();
        new_messages.push(Message {
            role: Role::User,
            content: user_msg,
            images: Vec::new(),
        });
        new_messages.push(Message {
            role: Role::Assistant,
            content: response.content,
            images: Vec::new(),
        });

        Ok(Value::conversation(Conversation {
            messages: new_messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/messages conv)
    register_fn(env, "conversation/messages", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/messages", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let msgs: Vec<Value> = conv
            .messages
            .iter()
            .map(|m| Value::message(m.clone()))
            .collect();
        Ok(Value::list(msgs))
    });

    // (conversation/last-reply conv)
    register_fn(env, "conversation/last-reply", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/last-reply", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        conv.messages
            .iter()
            .rfind(|m| m.role == Role::Assistant)
            .map(|m| Value::string(&m.content))
            .ok_or_else(|| SemaError::eval("no assistant reply in conversation"))
    });

    // (conversation/fork conv)
    register_fn(env, "conversation/fork", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/fork", "1", args.len()));
        }
        // Fork returns a copy - since conversations are immutable, this is just clone
        Ok(args[0].clone())
    });

    // Prompt functions

    // (prompt/append p1 p2 ...) — variadic, concatenates all prompts
    register_fn(env, "prompt/append", |args| {
        if args.is_empty() {
            return Err(SemaError::arity("prompt/append", "1+", args.len()));
        }
        let mut messages = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let p = arg
                .as_prompt_rc()
                .ok_or_else(|| SemaError::type_error("prompt", args[i].type_name()))?;
            messages.extend(p.messages.iter().cloned());
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/concat p1 p2 ...) — alias for variadic prompt/append
    register_fn(env, "prompt/concat", |args| {
        if args.is_empty() {
            return Err(SemaError::arity("prompt/concat", "1+", args.len()));
        }
        let mut messages = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let p = arg
                .as_prompt_rc()
                .ok_or_else(|| SemaError::type_error("prompt", args[i].type_name()))?;
            messages.extend(p.messages.iter().cloned());
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/messages prompt)
    register_fn(env, "prompt/messages", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("prompt/messages", "1", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let msgs: Vec<Value> = p
            .messages
            .iter()
            .map(|m| Value::message(m.clone()))
            .collect();
        Ok(Value::list(msgs))
    });

    // (prompt/set-system prompt "new system message")
    register_fn(env, "prompt/set-system", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/set-system", "2", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let new_system = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());
        let mut messages: Vec<Message> = p
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();
        messages.insert(
            0,
            Message {
                role: Role::System,
                content: new_system,
                images: Vec::new(),
            },
        );
        Ok(Value::prompt(Prompt { messages }))
    });

    // (message/role msg)
    register_fn(env, "message/role", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("message/role", "1", args.len()));
        }
        let msg = args[0]
            .as_message_rc()
            .ok_or_else(|| SemaError::type_error("message", args[0].type_name()))?;
        Ok(Value::keyword(&msg.role.to_string()))
    });

    // (message/content msg)
    register_fn(env, "message/content", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("message/content", "1", args.len()));
        }
        let msg = args[0]
            .as_message_rc()
            .ok_or_else(|| SemaError::type_error("message", args[0].type_name()))?;
        Ok(Value::string(&msg.content))
    });

    // Usage tracking

    // (llm/last-usage)
    register_fn(env, "llm/last-usage", |_args| {
        LAST_USAGE.with(|u| {
            let u = u.borrow();
            match &*u {
                Some(usage) => {
                    let mut map = BTreeMap::new();
                    map.insert(
                        Value::keyword("prompt-tokens"),
                        Value::int(usage.prompt_tokens as i64),
                    );
                    map.insert(
                        Value::keyword("completion-tokens"),
                        Value::int(usage.completion_tokens as i64),
                    );
                    map.insert(
                        Value::keyword("total-tokens"),
                        Value::int(usage.total_tokens() as i64),
                    );
                    map.insert(
                        Value::keyword("cache-read-tokens"),
                        Value::int(usage.cache_read_input_tokens as i64),
                    );
                    map.insert(
                        Value::keyword("cache-creation-tokens"),
                        Value::int(usage.cache_creation_input_tokens as i64),
                    );
                    map.insert(Value::keyword("model"), Value::string(&usage.model));
                    if let Some(cost) = pricing::calculate_cost(usage) {
                        map.insert(Value::keyword("cost-usd"), Value::float(cost));
                    }
                    Ok(Value::map(map))
                }
                None => Ok(Value::nil()),
            }
        })
    });

    // (llm/session-usage)
    register_fn(env, "llm/session-usage", |_args| {
        SESSION_USAGE.with(|u| {
            let usage = u.borrow();
            let mut map = BTreeMap::new();
            map.insert(
                Value::keyword("prompt-tokens"),
                Value::int(usage.prompt_tokens as i64),
            );
            map.insert(
                Value::keyword("completion-tokens"),
                Value::int(usage.completion_tokens as i64),
            );
            map.insert(
                Value::keyword("total-tokens"),
                Value::int(usage.total_tokens() as i64),
            );
            map.insert(
                Value::keyword("cache-read-tokens"),
                Value::int(usage.cache_read_input_tokens as i64),
            );
            map.insert(
                Value::keyword("cache-creation-tokens"),
                Value::int(usage.cache_creation_input_tokens as i64),
            );
            let session_cost = SESSION_COST.with(|sc| *sc.borrow());
            map.insert(Value::keyword("cost-usd"), Value::float(session_cost));
            Ok(Value::map(map))
        })
    });

    // (agent/run agent "msg") returns string
    // (agent/run agent "msg" {:on-tool-call cb :messages history}) returns {:response "..." :messages [...]}
    register_fn_ctx(env, "agent/run", |ctx, args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("agent/run", "2-3", args.len()));
        }
        let agent = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        let user_msg = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());

        // Extract options from 3rd arg
        let opts = args.get(2).and_then(|v| v.as_map_rc());

        let on_tool_call = opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("on-tool-call")).cloned());

        // Optional per-run reasoning effort, e.g. (agent/run a msg {:reasoning-effort :high}).
        let reasoning_effort = opts
            .as_ref()
            .and_then(|o| get_opt_effort(o, "reasoning-effort"));

        // Build messages: prior history + new user message
        let mut messages = if let Some(ref o) = opts {
            if let Some(history) = o.get(&Value::keyword("messages")) {
                sema_list_to_chat_messages(history)?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        messages.push(ChatMessage::new("user", user_msg));

        let tool_schemas = build_tool_schemas(&agent.tools)?;
        let system = if agent.system.is_empty() {
            None
        } else {
            Some(agent.system.clone())
        };

        let (result, final_messages) = run_tool_loop(
            ctx,
            messages,
            agent.model.clone(),
            Some(4096),
            None,
            system,
            reasoning_effort,
            &agent.tools,
            &tool_schemas,
            agent.max_turns,
            on_tool_call.as_ref(),
            Some(&agent.name),
        )?;

        // 3-arg form with opts: return {:response "..." :messages [...]}
        if opts.is_some() {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("response"), Value::string(&result));
            map.insert(
                Value::keyword("messages"),
                chat_messages_to_sema_list(&final_messages),
            );
            Ok(Value::map(map))
        } else {
            // 2-arg form: return string (backward compat)
            Ok(Value::string(&result))
        }
    });

    // (llm/pmap fn collection {:max-tokens N ...})
    // Maps fn over collection to produce prompts, then sends all prompts in parallel via batch_complete
    register_fn_ctx(env, "llm/pmap", |ctx, args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/pmap", "2-3", args.len()));
        }
        let func = &args[0];
        let items = args[1]
            .as_seq()
            .map(|l| l.to_vec())
            .ok_or_else(|| SemaError::type_error("list or vector", args[1].type_name()))?;

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;

        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                max_tokens = get_opt_u32(&opts, "max-tokens");
                temperature = get_opt_f64(&opts, "temperature");
                system = get_opt_string(&opts, "system");
            }
        }

        // Step 1: Map fn over items to produce prompt strings (sequentially, since Rc)
        let mut prompts = Vec::with_capacity(items.len());
        for item in &items {
            #[allow(clippy::cloned_ref_to_slice_refs)] // clone needed: &Value -> [Value]
            let result = call_value_fn(ctx, func, &[item.clone()])?;
            let prompt_str = result
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| result.to_string());
            prompts.push(prompt_str);
        }

        // Step 2: Build ChatRequests
        let requests: Vec<ChatRequest> = prompts
            .into_iter()
            .map(|prompt_text| {
                let messages = vec![ChatMessage::new("user", prompt_text)];
                let mut req = ChatRequest::new(model.clone(), messages);
                req.max_tokens = max_tokens.or(Some(4096));
                req.temperature = temperature;
                req.system = system.clone();
                req
            })
            .collect();

        // Step 3: batch_complete (runs concurrently at provider level)
        let responses = with_provider(|p| {
            let reqs: Vec<ChatRequest> = requests
                .into_iter()
                .map(|mut r| {
                    if r.model.is_empty() {
                        r.model = p.default_model().to_string();
                    }
                    r
                })
                .collect();
            Ok(p.batch_complete(reqs))
        })?;

        // Step 4: Collect results
        let mut results = Vec::with_capacity(responses.len());
        for resp_result in responses {
            let resp = resp_result.map_err(|e| SemaError::Llm(e.to_string()))?;
            track_usage(&resp.usage)?;
            results.push(Value::string(&resp.content));
        }
        Ok(Value::list(results))
    });

    // (llm/batch ["prompt1" "prompt2" "prompt3"] {:max-tokens 100})
    register_fn(env, "llm/batch", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/batch", "1-2", args.len()));
        }
        let prompts = args[0]
            .as_seq()
            .map(|l| l.to_vec())
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;

        if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                max_tokens = get_opt_u32(&opts, "max-tokens");
                temperature = get_opt_f64(&opts, "temperature");
                system = get_opt_string(&opts, "system");
            }
        }

        let requests: Vec<ChatRequest> = prompts
            .iter()
            .map(|prompt_val| {
                let prompt_text = prompt_val
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| prompt_val.to_string());
                let messages = vec![ChatMessage::new("user", prompt_text)];
                let mut req = ChatRequest::new(model.clone(), messages);
                req.max_tokens = max_tokens.or(Some(4096));
                req.temperature = temperature;
                req.system = system.clone();
                req
            })
            .collect();

        let responses = with_provider(|p| {
            let reqs: Vec<ChatRequest> = requests
                .into_iter()
                .map(|mut r| {
                    if r.model.is_empty() {
                        r.model = p.default_model().to_string();
                    }
                    r
                })
                .collect();
            Ok(p.batch_complete(reqs))
        })?;

        let mut results = Vec::with_capacity(responses.len());
        for resp_result in responses {
            let resp = resp_result.map_err(|e| SemaError::Llm(e.to_string()))?;
            track_usage(&resp.usage)?;
            results.push(Value::string(&resp.content));
        }
        Ok(Value::list(results))
    });

    // (llm/set-pricing "model-pattern" input-per-million output-per-million)
    register_fn(env, "llm/set-pricing", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity("llm/set-pricing", "3", args.len()));
        }
        let model_pattern = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let input_cost = args[1]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        let output_cost = args[2]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[2].type_name()))?;
        pricing::set_custom_pricing(model_pattern, input_cost, output_cost);
        Ok(Value::nil())
    });

    // (llm/configure-embeddings :openai {:api-key "..." :base-url "..." :model "..."})
    // (llm/configure-embeddings :jina {:api-key "..."})
    // (llm/configure-embeddings :voyage {:api-key "..."})
    // (llm/configure-embeddings :cohere {:api-key "..."})
    register_fn(env, "llm/configure-embeddings", move |args| {
        if args.len() != 2 {
            return Err(SemaError::arity(
                "llm/configure-embeddings",
                "2",
                args.len(),
            ));
        }
        let provider_name = args[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
        let opts_rc = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let opts = opts_rc.as_ref().clone();

        guard_provider_url(unrestricted, &opts)?;

        let api_key = get_opt_string(&opts, "api-key");

        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            match provider_name.as_str() {
                "jina" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "jina-embeddings-v3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        api_key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                }
                "voyage" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "voyage-3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        api_key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("voyage");
                }
                "cohere" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = CohereEmbeddingProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("cohere");
                }
                _ => {
                    // Default: OpenAI-compatible
                    let api_key = api_key.unwrap_or_default();
                    let base_url = get_opt_string(&opts, "base-url");
                    let model = get_opt_string(&opts, "default-model")
                        .or_else(|| get_opt_string(&opts, "model"));
                    let provider = OpenAiProvider::new(api_key, base_url, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    let name = provider.name().to_string();
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider(&name);
                }
            }
            Ok(Value::nil())
        })
    });

    // (llm/embed "text" {:model "..."})
    // (llm/embed ["text1" "text2"] {:model "..."})
    register_fn(env, "llm/embed", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/embed", "1-2", args.len()));
        }

        let (texts, single) = if let Some(s) = args[0].as_str() {
            (vec![s.to_string()], true)
        } else if let Some(l) = args[0].as_seq() {
            let texts: Vec<String> = l
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            (texts, false)
        } else {
            return Err(SemaError::type_error("string or list", args[0].type_name()));
        };

        let model = if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                get_opt_string(&opts, "model")
            } else {
                None
            }
        } else {
            None
        };

        let request = EmbedRequest { texts, model };
        let response = with_embedding_provider(|p| {
            p.embed(request).map_err(|e| SemaError::Llm(e.to_string()))
        })?;

        track_usage(&response.usage)?;

        if single {
            let embedding = response.embeddings.into_iter().next().unwrap_or_default();
            let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
            Ok(Value::bytevector(bytes))
        } else {
            Ok(Value::list(
                response
                    .embeddings
                    .into_iter()
                    .map(|emb| {
                        let bytes: Vec<u8> = emb.iter().flat_map(|f| f.to_le_bytes()).collect();
                        Value::bytevector(bytes)
                    })
                    .collect(),
            ))
        }
    });

    // (llm/similarity vec1 vec2) — cosine similarity
    register_fn(env, "llm/similarity", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/similarity", "2", args.len()));
        }

        let a_is_bv = args[0].as_bytevector().is_some();
        let b_is_bv = args[1].as_bytevector().is_some();
        let a_is_list = args[0].as_seq().is_some();
        let b_is_list = args[1].as_seq().is_some();

        if a_is_bv && b_is_bv {
            let ba = args[0].as_bytevector().unwrap();
            let bb = args[1].as_bytevector().unwrap();
            if ba.len() != bb.len() {
                return Err(SemaError::eval(format!(
                    "llm/similarity: bytevectors must have same length ({} vs {})",
                    ba.len(),
                    bb.len()
                )));
            }
            if ba.is_empty() {
                return Err(SemaError::eval("llm/similarity: empty vectors"));
            }
            if ba.len() % 8 != 0 {
                return Err(SemaError::eval(format!(
                    "llm/similarity: bytevector length must be a multiple of 8 (got {})",
                    ba.len()
                )));
            }
            let mut dot = 0.0_f64;
            let mut mag_a = 0.0_f64;
            let mut mag_b = 0.0_f64;
            for (ca, cb) in ba.chunks_exact(8).zip(bb.chunks_exact(8)) {
                let fa = f64::from_le_bytes(ca.try_into().unwrap());
                let fb = f64::from_le_bytes(cb.try_into().unwrap());
                dot += fa * fb;
                mag_a += fa * fa;
                mag_b += fb * fb;
            }
            if mag_a == 0.0 || mag_b == 0.0 {
                Ok(Value::float(0.0))
            } else {
                Ok(Value::float(dot / (mag_a.sqrt() * mag_b.sqrt())))
            }
        } else if a_is_list && b_is_list {
            let va = extract_float_vec(&args[0])?;
            let vb = extract_float_vec(&args[1])?;
            if va.len() != vb.len() {
                return Err(SemaError::eval(format!(
                    "llm/similarity: vectors must have same length ({} vs {})",
                    va.len(),
                    vb.len()
                )));
            }
            if va.is_empty() {
                return Err(SemaError::eval("llm/similarity: empty vectors"));
            }
            let mut dot = 0.0_f64;
            let mut mag_a = 0.0_f64;
            let mut mag_b = 0.0_f64;
            for i in 0..va.len() {
                dot += va[i] * vb[i];
                mag_a += va[i] * va[i];
                mag_b += vb[i] * vb[i];
            }
            if mag_a == 0.0 || mag_b == 0.0 {
                Ok(Value::float(0.0))
            } else {
                Ok(Value::float(dot / (mag_a.sqrt() * mag_b.sqrt())))
            }
        } else {
            Err(SemaError::eval(
                "llm/similarity: both arguments must be the same type (both bytevectors or both lists). \
                 Use embedding/->list or embedding/list->embedding to convert between formats.",
            ))
        }
    });

    register_fn(env, "embedding/length", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("embedding/length", "1", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/length: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        Ok(Value::int((bv.len() / 8) as i64))
    });

    register_fn(env, "embedding/ref", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("embedding/ref", "2", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/ref: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        let idx = idx as usize;
        let num_elements = bv.len() / 8;
        if idx >= num_elements {
            return Err(SemaError::eval(format!(
                "embedding/ref: index {} out of bounds (length {})",
                idx, num_elements
            )));
        }
        let start = idx * 8;
        let bytes: [u8; 8] = bv[start..start + 8].try_into().unwrap();
        Ok(Value::float(f64::from_le_bytes(bytes)))
    });

    register_fn(env, "embedding/->list", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("embedding/->list", "1", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/->list: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        let floats: Vec<Value> = bv
            .chunks_exact(8)
            .map(|chunk| {
                let bytes: [u8; 8] = chunk.try_into().unwrap();
                Value::float(f64::from_le_bytes(bytes))
            })
            .collect();
        Ok(Value::list(floats))
    });

    register_fn(env, "embedding/list->embedding", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "embedding/list->embedding",
                "1",
                args.len(),
            ));
        }
        let items = args[0]
            .as_seq()
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;
        let mut bytes = Vec::with_capacity(items.len() * 8);
        for (i, item) in items.iter().enumerate() {
            let f = item.as_float().ok_or_else(|| {
                SemaError::eval(format!(
                    "embedding/list->embedding: element {} is {}, expected number",
                    i,
                    item.type_name()
                ))
            })?;
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        Ok(Value::bytevector(bytes))
    });

    register_fn(env, "llm/reset-usage", |_args| {
        SESSION_USAGE.with(|u| *u.borrow_mut() = Usage::default());
        LAST_USAGE.with(|u| *u.borrow_mut() = None);
        SESSION_COST.with(|sc| *sc.borrow_mut() = 0.0);
        Ok(Value::nil())
    });

    // Type predicates for LLM types
    register_fn(env, "prompt?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("prompt?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_prompt_rc().is_some()))
    });

    register_fn(env, "message?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("message?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_message_rc().is_some()))
    });

    // (message/with-image :user "Describe this" bytevec)
    // (message/with-image :user "Describe this" bytevec {:media-type "image/png"})
    register_fn(env, "message/with-image", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("message/with-image", "3-4", args.len()));
        }
        let role = if let Some(kw) = args[0].as_keyword() {
            match kw.as_str() {
                "system" => Role::System,
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                other => {
                    return Err(SemaError::eval(format!(
                        "message/with-image: unknown role '{other}'"
                    )))
                }
            }
        } else {
            return Err(SemaError::type_error("keyword", args[0].type_name()));
        };
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();
        let bv = args[2]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[2].type_name()))?;

        let media_type = if let Some(opts) = args.get(3).and_then(|v| v.as_map_rc()) {
            opts.get(&Value::keyword("media-type"))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| detect_media_type(bv).to_string())
        } else {
            detect_media_type(bv).to_string()
        };

        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(bv);

        Ok(Value::message(Message {
            role,
            content: text,
            images: vec![ImageAttachment { data, media_type }],
        }))
    });

    register_fn(env, "conversation?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_conversation_rc().is_some()))
    });

    register_fn(env, "tool?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_tool_def_rc().is_some()))
    });

    register_fn(env, "agent?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_agent_rc().is_some()))
    });

    // Tool accessor functions
    register_fn(env, "tool/name", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/name", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(Value::string(&t.name))
    });

    register_fn(env, "tool/description", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/description", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(Value::string(&t.description))
    });

    register_fn(env, "tool/parameters", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/parameters", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(t.parameters.clone())
    });

    // Agent accessor functions
    register_fn(env, "agent/name", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/name", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.name))
    });

    register_fn(env, "agent/system", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/system", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.system))
    });

    register_fn(env, "agent/tools", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/tools", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::list(a.tools.clone()))
    });

    register_fn(env, "agent/model", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/model", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.model))
    });

    register_fn(env, "agent/max-turns", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/max-turns", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::int(a.max_turns as i64))
    });

    // (conversation/add-message conv :role "content")
    register_fn(env, "conversation/add-message", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity(
                "conversation/add-message",
                "3",
                args.len(),
            ));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let role_kw = args[1]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[1].type_name()))?;
        let role = match role_kw.as_str() {
            "system" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            other => {
                return Err(SemaError::eval(format!(
                    "conversation/add-message: unknown role '{other}'"
                )))
            }
        };
        let content = args[2]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[2].to_string());
        let mut new_messages = conv.messages.clone();
        new_messages.push(Message {
            role,
            content,
            images: Vec::new(),
        });
        Ok(Value::conversation(Conversation {
            messages: new_messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/model conv) — get the model name
    register_fn(env, "conversation/model", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/model", "1", args.len()));
        }
        let c = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        Ok(Value::string(&c.model))
    });

    // (conversation/system conv) — get the system message content, or nil
    register_fn(env, "conversation/system", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/system", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        Ok(conv
            .messages
            .iter()
            .find(|m| m.role == Role::System)
            .map(|m| Value::string(&m.content))
            .unwrap_or_else(Value::nil))
    });

    // (conversation/set-system conv "new system message") — set/replace the system message
    register_fn(env, "conversation/set-system", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/set-system", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let new_system = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());
        let mut messages: Vec<Message> = conv
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();
        messages.insert(
            0,
            Message {
                role: Role::System,
                content: new_system,
                images: Vec::new(),
            },
        );
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/filter conv pred) — keep only messages where (pred msg) is truthy
    register_fn_ctx(env, "conversation/filter", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/filter", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let pred = &args[1];
        let mut filtered = Vec::new();
        for msg in &conv.messages {
            let msg_val = Value::message(msg.clone());
            let result = sema_core::call_callback(ctx, pred, &[msg_val])?;
            if result.is_truthy() {
                filtered.push(msg.clone());
            }
        }
        Ok(Value::conversation(Conversation {
            messages: filtered,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/map conv f) — transform each message with (f msg), returns list of results
    register_fn_ctx(env, "conversation/map", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/map", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let func = &args[1];
        let mut results = Vec::new();
        for msg in &conv.messages {
            let msg_val = Value::message(msg.clone());
            let result = sema_core::call_callback(ctx, func, &[msg_val])?;
            results.push(result);
        }
        Ok(Value::list(results))
    });

    // (conversation/say-as conv system-prompt "message" opts?) — say with a different system prompt for one turn
    register_fn(env, "conversation/say-as", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("conversation/say-as", "3-4", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;

        // Second arg: either a prompt value (use its system messages) or a string
        let system_override = if let Some(p) = args[1].as_prompt_rc() {
            p.messages
                .iter()
                .filter(|m| m.role == Role::System)
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else if let Some(s) = args[1].as_str() {
            s.to_string()
        } else {
            return Err(SemaError::type_error(
                "prompt or string",
                args[1].type_name(),
            ));
        };

        let user_msg = args[2]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[2].to_string());

        // Parse optional opts
        let mut temperature = None;
        let mut max_tokens = None;
        if let Some(opts_val) = args.get(3) {
            if let Some(opts) = opts_val.as_map_rc() {
                temperature = get_opt_f64(&opts, "temperature");
                max_tokens = get_opt_u32(&opts, "max-tokens");
            }
        }

        // Build messages for API call — use the system override instead of any existing system msg
        let mut chat_messages: Vec<ChatMessage> = conv
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
            .collect();
        chat_messages.push(ChatMessage::new("user", user_msg.clone()));

        let mut request = ChatRequest::new(conv.model.clone(), chat_messages);
        request.temperature = temperature;
        request.max_tokens = max_tokens.or(Some(4096));
        request.system = Some(system_override);

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

        // Build new conversation preserving the original system message (not the override)
        let mut new_messages = conv.messages.clone();
        new_messages.push(Message {
            role: Role::User,
            content: user_msg,
            images: Vec::new(),
        });
        new_messages.push(Message {
            role: Role::Assistant,
            content: response.content,
            images: Vec::new(),
        });

        Ok(Value::conversation(Conversation {
            messages: new_messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/token-count conv) — count total tokens in conversation messages
    register_fn(env, "conversation/token-count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "conversation/token-count",
                "1",
                args.len(),
            ));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        // Approximate: ~4 chars per token (common heuristic)
        let total_chars: usize = conv.messages.iter().map(|m| m.content.len()).sum();
        let estimated_tokens = (total_chars as f64 / 4.0).ceil() as i64;
        Ok(Value::int(estimated_tokens))
    });

    // (conversation/cost conv) — estimate cost based on token count and model
    register_fn(env, "conversation/cost", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/cost", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        // Approximate token counts
        let total_chars: usize = conv.messages.iter().map(|m| m.content.len()).sum();
        let estimated_tokens = (total_chars as f64 / 4.0).ceil() as u32;
        // Split: all messages are input tokens (the full context for next call)
        let usage = Usage {
            prompt_tokens: estimated_tokens,
            completion_tokens: 0,
            model: conv.model.clone(),
            ..Default::default()
        };
        match pricing::calculate_cost(&usage) {
            Some(cost) => Ok(Value::float(cost)),
            None => Ok(Value::nil()),
        }
    });

    // (prompt/fill prompt vars-map) — substitute {{key}} in all message contents
    register_fn(env, "prompt/fill", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/fill", "2", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let vars = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let messages: Vec<Message> = p
            .messages
            .iter()
            .map(|m| {
                let filled = fill_template(&m.content, &vars);
                Message {
                    role: m.role.clone(),
                    content: filled,
                    images: m.images.clone(),
                }
            })
            .collect();
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/slots prompt) — return list of unfilled {{slot}} names
    register_fn(env, "prompt/slots", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("prompt/slots", "1", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let mut slots = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for m in &p.messages {
            let mut chars = m.content.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '{' && chars.peek() == Some(&'{') {
                    chars.next();
                    let mut name = String::new();
                    let mut found_close = false;
                    while let Some(c) = chars.next() {
                        if c == '}' && chars.peek() == Some(&'}') {
                            chars.next();
                            found_close = true;
                            break;
                        }
                        name.push(c);
                    }
                    if found_close && !name.is_empty() && seen.insert(name.clone()) {
                        slots.push(Value::keyword(&name));
                    }
                }
            }
        }
        Ok(Value::list(slots))
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

    // (llm/set-budget max-cost-usd) — set a budget limit
    register_fn(env, "llm/set-budget", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/set-budget", "1", args.len()));
        }
        let max_cost = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        crate::builtins::set_budget(max_cost);
        Ok(Value::nil())
    });

    // (llm/clear-budget) — clear the budget limit
    register_fn(env, "llm/clear-budget", |_args| {
        crate::builtins::clear_budget();
        Ok(Value::nil())
    });

    // (llm/budget-remaining) — query budget status
    register_fn(env, "llm/budget-remaining", |_args| {
        let cost_limit = BUDGET_LIMIT.with(|l| *l.borrow());
        let token_limit = BUDGET_TOKEN_LIMIT.with(|l| *l.borrow());
        if cost_limit.is_none() && token_limit.is_none() {
            return Ok(Value::nil());
        }
        let mut map = BTreeMap::new();
        if let Some(max_cost) = cost_limit {
            let spent = BUDGET_SPENT.with(|s| *s.borrow());
            map.insert(Value::keyword("limit"), Value::float(max_cost));
            map.insert(Value::keyword("spent"), Value::float(spent));
            map.insert(Value::keyword("remaining"), Value::float(max_cost - spent));
        }
        if let Some(max_tokens) = token_limit {
            let tokens_spent = BUDGET_TOKENS_SPENT.with(|s| *s.borrow());
            map.insert(Value::keyword("token-limit"), Value::int(max_tokens as i64));
            map.insert(
                Value::keyword("tokens-spent"),
                Value::int(tokens_spent as i64),
            );
            map.insert(
                Value::keyword("tokens-remaining"),
                Value::int((max_tokens.saturating_sub(tokens_spent)) as i64),
            );
        }
        Ok(Value::map(map))
    });

    // (llm/with-budget {:max-cost-usd 0.50 :max-tokens 10000} thunk)
    register_fn_ctx(env, "llm/with-budget", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-budget", "2", args.len()));
        }
        let opts = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }

        let max_cost = opts
            .get(&Value::keyword("max-cost-usd"))
            .and_then(|v| v.as_float());
        let max_tokens = opts
            .get(&Value::keyword("max-tokens"))
            .and_then(|v| v.as_int())
            .map(|v| v.max(0) as u64);

        if max_cost.is_none() && max_tokens.is_none() {
            return Err(SemaError::eval(
                "llm/with-budget: requires at least :max-cost-usd or :max-tokens",
            ));
        }

        push_budget_scope(max_cost, max_tokens);
        let result = call_value_fn(ctx, body_fn, &[]);
        pop_budget_scope();
        result
    });

    // --- Cache builtins ---

    register_fn(env, "llm/cache-key", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/cache-key", "1-2", args.len()));
        }
        let prompt = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut model = String::new();
        let mut temperature = None;
        let mut system = None;
        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            model = get_opt_string(&opts, "model").unwrap_or_default();
            temperature = get_opt_f64(&opts, "temperature");
            system = get_opt_string(&opts, "system");
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

    register_fn_ctx(env, "llm/with-cache", |ctx, args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/with-cache", "1-2", args.len()));
        }
        let (body_fn, ttl) = if args.len() == 2 {
            let opts = args[0]
                .as_map_rc()
                .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
            let ttl = get_opt_u32(&opts, "ttl").unwrap_or(3600) as i64;
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
        let result = call_value_fn(ctx, body_fn, &[]);
        CACHE_ENABLED.with(|c| c.set(prev_enabled));
        CACHE_TTL_SECS.with(|c| c.set(prev_ttl));
        result
    });

    // --- Fallback provider builtins ---

    register_fn_ctx(env, "llm/with-fallback", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-fallback", "2", args.len()));
        }
        let providers = args[0]
            .as_seq()
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let chain: Vec<FallbackEntry> = providers
            .iter()
            .map(parse_fallback_entry)
            .collect::<Result<_, _>>()?;
        let prev = FALLBACK_CHAIN.with(|c| c.borrow().clone());
        FALLBACK_CHAIN.with(|c| *c.borrow_mut() = Some(chain));
        let result = call_value_fn(ctx, body_fn, &[]);
        FALLBACK_CHAIN.with(|c| *c.borrow_mut() = prev);
        result
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

    register_fn(env, "vector-store/create", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector-store/create", "1", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        VECTOR_STORES.with(|s| s.borrow_mut().insert(name.to_string(), VectorStore::new()));
        Ok(Value::string(name))
    });

    register_fn(env, "vector-store/add", |args| {
        if args.len() != 4 {
            return Err(SemaError::arity("vector-store/add", "4", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let id = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let emb = args[2]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[2].type_name()))?;
        if emb.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "vector-store/add: embedding length {} not multiple of 8",
                emb.len()
            )));
        }
        let metadata = args[3].clone();
        VECTOR_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s
                .get_mut(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            store.add(VectorDocument {
                id: id.to_string(),
                embedding: emb.to_vec(),
                metadata,
            });
            Ok(Value::string(id))
        })
    });

    register_fn(env, "vector-store/search", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity("vector-store/search", "3", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let query = args[1]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
        let k = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))?
            as usize;
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            Ok(Value::list(
                store
                    .search(query, k)?
                    .iter()
                    .map(|r| r.to_value())
                    .collect(),
            ))
        })
    });

    register_fn(env, "vector-store/delete", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("vector-store/delete", "2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let id = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        VECTOR_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s
                .get_mut(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            Ok(Value::bool(store.delete(id)))
        })
    });

    register_fn(env, "vector-store/count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector-store/count", "1", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            Ok(Value::int(store.count() as i64))
        })
    });

    // (vector-store/save name) or (vector-store/save name path)
    register_fn(env, "vector-store/save", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("vector-store/save", "1-2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let explicit_path = args.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            let path = explicit_path
                .as_deref()
                .or(store.path.as_deref())
                .ok_or_else(|| {
                    SemaError::eval(
                        "vector-store/save: no path associated. Use (vector-store/save name path)",
                    )
                })?;
            let data = store.to_json().map_err(SemaError::Io)?;
            let tmp = format!("{path}.tmp");
            std::fs::write(&tmp, &data)
                .map_err(|e| SemaError::Io(format!("vector-store/save: {e}")))?;
            std::fs::rename(&tmp, path)
                .map_err(|e| SemaError::Io(format!("vector-store/save: {e}")))?;
            Ok(Value::string(path))
        })
    });

    // (vector-store/open name path) — load from disk or create empty, associate path
    register_fn(env, "vector-store/open", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("vector-store/open", "2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let path = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let mut store = if std::path::Path::new(path).exists() {
            let data = std::fs::read(path)
                .map_err(|e| SemaError::Io(format!("vector-store/open: {e}")))?;
            VectorStore::from_json(&data)
                .map_err(|e| SemaError::Io(format!("vector-store/open: {e}")))?
        } else {
            VectorStore::new()
        };
        store.path = Some(path.to_string());
        VECTOR_STORES.with(|s| s.borrow_mut().insert(name.to_string(), store));
        Ok(Value::string(name))
    });

    // --- Vector math builtins ---

    register_fn(env, "vector/cosine-similarity", |args| {
        let (a, b) = require_matching_bytevectors("vector/cosine-similarity", args)?;
        let (mut dot, mut ma, mut mb) = (0.0_f64, 0.0_f64, 0.0_f64);
        for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
            let (fa, fb) = (
                f64::from_le_bytes(ca.try_into().unwrap()),
                f64::from_le_bytes(cb.try_into().unwrap()),
            );
            dot += fa * fb;
            ma += fa * fa;
            mb += fb * fb;
        }
        Ok(Value::float(if ma == 0.0 || mb == 0.0 {
            0.0
        } else {
            dot / (ma.sqrt() * mb.sqrt())
        }))
    });

    register_fn(env, "vector/dot-product", |args| {
        let (a, b) = require_matching_bytevectors("vector/dot-product", args)?;
        let mut dot = 0.0_f64;
        for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
            dot += f64::from_le_bytes(ca.try_into().unwrap())
                * f64::from_le_bytes(cb.try_into().unwrap());
        }
        Ok(Value::float(dot))
    });

    register_fn(env, "vector/normalize", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector/normalize", "1", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        if bv.is_empty() || bv.len() % 8 != 0 {
            return Err(SemaError::eval("vector/normalize: invalid bytevector"));
        }
        let floats: Vec<f64> = bv
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let mag: f64 = floats.iter().map(|f| f * f).sum::<f64>().sqrt();
        let out: Vec<u8> = if mag == 0.0 {
            floats.iter().flat_map(|_| 0.0_f64.to_le_bytes()).collect()
        } else {
            floats
                .iter()
                .flat_map(|f| (f / mag).to_le_bytes())
                .collect()
        };
        Ok(Value::bytevector(out))
    });

    register_fn(env, "vector/distance", |args| {
        let (a, b) = require_matching_bytevectors("vector/distance", args)?;
        let mut sum_sq = 0.0_f64;
        for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
            let d = f64::from_le_bytes(ca.try_into().unwrap())
                - f64::from_le_bytes(cb.try_into().unwrap());
            sum_sq += d * d;
        }
        Ok(Value::float(sum_sq.sqrt()))
    });

    // --- Rate limiting ---

    register_fn_ctx(env, "llm/with-rate-limit", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-rate-limit", "2", args.len()));
        }
        let rps = args[0]
            .as_float()
            .or_else(|| args[0].as_int().map(|i| i as f64))
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let prev = RATE_LIMIT_RPS.with(|r| r.get());
        RATE_LIMIT_RPS.with(|r| r.set(Some(rps)));
        let result = call_value_fn(ctx, body_fn, &[]);
        RATE_LIMIT_RPS.with(|r| r.set(prev));
        result
    });

    // --- Convenience wrappers ---

    register_fn(env, "llm/summarize", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/summarize", "1-2", args.len()));
        }
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        let mut model = String::new();
        let mut max_length: Option<u32> = None;
        let mut style = "paragraph".to_string();

        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            model = get_opt_string(&opts, "model").unwrap_or_default();
            max_length = get_opt_u32(&opts, "max-length");
            if let Some(s) = get_opt_string(&opts, "style") {
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

        let response = do_complete(request)?;
        track_usage(&response.usage)?;
        Ok(Value::string(&response.content))
    });

    register_fn(env, "llm/compare", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/compare", "2-3", args.len()));
        }
        let text_a = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let text_b = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let mut model = String::new();
        if let Some(opts) = args.get(2).and_then(|v| v.as_map_rc()) {
            model = get_opt_string(&opts, "model").unwrap_or_default();
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

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

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
    });
}

fn require_matching_bytevectors<'a>(
    name: &str,
    args: &'a [Value],
) -> Result<(&'a [u8], &'a [u8]), SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity(name, "2", args.len()));
    }
    let a = args[0]
        .as_bytevector()
        .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
    let b = args[1]
        .as_bytevector()
        .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
    if a.len() != b.len() {
        return Err(SemaError::eval(format!(
            "{name}: length mismatch ({} vs {})",
            a.len() / 8,
            b.len() / 8
        )));
    }
    if a.is_empty() || a.len() % 8 != 0 {
        return Err(SemaError::eval(format!(
            "{name}: invalid bytevector length {}",
            a.len()
        )));
    }
    Ok((a, b))
}

fn extract_float_vec(val: &Value) -> Result<Vec<f64>, SemaError> {
    let items = val
        .as_seq()
        .ok_or_else(|| SemaError::type_error("list of numbers", val.type_name()))?;
    items
        .iter()
        .map(|v| {
            v.as_float()
                .ok_or_else(|| SemaError::type_error("number", v.type_name()))
        })
        .collect()
}

fn complete_with_prompt(prompt: &Prompt, opts: Option<&Value>) -> Result<Value, SemaError> {
    let messages: Vec<ChatMessage> = prompt
        .messages
        .iter()
        .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
        .collect();

    let mut model = String::new();
    let mut max_tokens = None;
    let mut temperature = None;

    if let Some(opts) = opts.and_then(|v| v.as_map_rc()) {
        model = get_opt_string(&opts, "model").unwrap_or_default();
        max_tokens = get_opt_u32(&opts, "max-tokens");
        temperature = get_opt_f64(&opts, "temperature");
    }

    let mut request = ChatRequest::new(model, messages);
    request.max_tokens = max_tokens.or(Some(4096));
    request.temperature = temperature;

    let response = do_complete(request)?;
    track_usage(&response.usage)?;
    Ok(Value::string(&response.content))
}

fn message_to_chat_message(m: &Message) -> ChatMessage {
    if m.images.is_empty() {
        ChatMessage::new(m.role.to_string(), m.content.clone())
    } else {
        let mut blocks = Vec::new();
        for img in &m.images {
            blocks.push(ContentBlock::Image {
                media_type: Some(img.media_type.clone()),
                data: img.data.clone(),
            });
        }
        blocks.push(ContentBlock::Text {
            text: m.content.clone(),
        });
        ChatMessage::with_blocks(m.role.to_string(), blocks)
    }
}

fn extract_messages(val: &Value) -> Result<Vec<ChatMessage>, SemaError> {
    if let Some(items) = val.as_seq() {
        let mut messages = Vec::new();
        for item in items.iter() {
            let m = item
                .as_message_rc()
                .ok_or_else(|| SemaError::type_error("message", item.type_name()))?;
            messages.push(message_to_chat_message(&m));
        }
        Ok(messages)
    } else if let Some(p) = val.as_prompt_rc() {
        Ok(p.messages.iter().map(message_to_chat_message).collect())
    } else {
        Err(SemaError::type_error(
            "list of messages or prompt",
            val.type_name(),
        ))
    }
}

fn format_reask_prompt(prev_response: &str, errors: &str, schema_desc: &str) -> String {
    format!(
        "Your previous response did not match the required schema.\n\n\
         Previous response:\n```json\n{prev_response}\n```\n\n\
         Validation errors:\n{errors}\n\n\
         Please respond with ONLY a corrected JSON object matching this schema:\n\
         {schema_desc}\nDo not include any other text."
    )
}

fn format_schema(val: &Value) -> String {
    if let Some(map) = val.as_map_rc() {
        let mut fields = Vec::new();
        for (k, v) in map.iter() {
            let key = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            let type_str = if let Some(inner) = v.as_map_rc() {
                if let Some(t) = inner.get(&Value::keyword("type")) {
                    t.as_keyword()
                        .or_else(|| t.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| t.to_string())
                } else {
                    "any".to_string()
                }
            } else {
                "any".to_string()
            };
            fields.push(format!("  \"{key}\": <{type_str}>"));
        }
        format!("{{\n{}\n}}", fields.join(",\n"))
    } else {
        val.to_string()
    }
}

/// Validate that an extracted Sema value matches the expected schema.
/// The schema is a map of keyword keys to field descriptors (maps with `:type`).
/// Returns Ok(()) if valid, or Err with a description of mismatches.
fn validate_extraction(result: &Value, schema: &Value) -> Result<(), String> {
    let schema_map = match schema.as_map_rc() {
        Some(m) => m,
        None => return Ok(()),
    };
    let result_map = match result.as_map_rc() {
        Some(m) => m,
        None => return Err(format!("expected map result, got {}", result.type_name())),
    };

    let mut errors = Vec::new();

    for (key, field_spec) in schema_map.iter() {
        let key_name = key
            .as_keyword()
            .or_else(|| key.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| key.to_string());

        // Check if field is optional (only applies to map-style field specs)
        let is_optional = if let Some(spec) = field_spec.as_map_rc() {
            spec.get(&Value::keyword("optional"))
                .map(|v| v.is_truthy())
                .unwrap_or(false)
        } else {
            false
        };

        let result_val = result_map.get(key);
        match result_val {
            None => {
                if !is_optional {
                    errors.push(format!("missing key: {key_name}"));
                }
            }
            Some(val) => {
                if let Some(spec) = field_spec.as_map_rc() {
                    // Type checking
                    if let Some(type_val) = spec.get(&Value::keyword("type")) {
                        let type_name = type_val
                            .as_keyword()
                            .or_else(|| type_val.as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| type_val.to_string());
                        let ok = match type_name.as_str() {
                            "string" => val.as_str().is_some(),
                            "number" => val.as_float().is_some(),
                            "boolean" | "bool" => val.as_bool().is_some(),
                            "list" | "array" => val.as_seq().is_some(),
                            _ => true,
                        };
                        if !ok {
                            errors.push(format!(
                                "key {key_name}: expected {type_name}, got {}",
                                val.type_name()
                            ));
                            continue; // skip :validate if type check failed
                        }
                    }

                    // Custom predicate validation via :validate
                    if let Some(validate_fn) = spec.get(&Value::keyword("validate")) {
                        let custom_msg = spec
                            .get(&Value::keyword("message"))
                            .and_then(|v| v.as_str().map(|s| s.to_string()));

                        match sema_core::with_stdlib_ctx(|ctx| {
                            sema_core::call_callback(ctx, validate_fn, std::slice::from_ref(val))
                        }) {
                            Ok(v) if v.is_truthy() => {} // validation passed
                            Ok(_) => {
                                let msg = custom_msg.unwrap_or_else(|| {
                                    format!("custom validation failed for value {}", val)
                                });
                                errors.push(format!("key {key_name}: {msg}"));
                            }
                            Err(e) => {
                                errors.push(format!("key {key_name}: validation error: {e}"));
                            }
                        }
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn compute_cache_key(request: &ChatRequest) -> String {
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
    format!("{:x}", hasher.finalize())
}

fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn cache_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".sema")
        .join("cache")
        .join("llm")
}

fn cache_file_path(key: &str) -> std::path::PathBuf {
    cache_dir().join(format!("{key}.json"))
}

fn load_cached(key: &str) -> Option<CachedResponse> {
    let mem_hit = CACHE_MEM.with(|c| c.borrow().get(key).cloned());
    if let Some(cached) = mem_hit {
        return Some(cached);
    }
    let path = cache_file_path(key);
    let data = std::fs::read_to_string(&path).ok()?;
    let cached: CachedResponse = serde_json::from_str(&data).ok()?;
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    Some(cached)
}

fn store_cached(key: &str, response: &ChatResponse) {
    let cached = CachedResponse {
        content: response.content.clone(),
        model: response.model.clone(),
        prompt_tokens: response.usage.prompt_tokens,
        completion_tokens: response.usage.completion_tokens,
        cached_at: unix_timestamp(),
    };
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = std::fs::write(cache_file_path(key), json);
    }
}

fn is_cache_valid(cached: &CachedResponse) -> bool {
    let ttl = CACHE_TTL_SECS.with(|c| c.get());
    (unix_timestamp() - cached.cached_at) < ttl
}

/// Send a ChatRequest via the default provider with caching, fallback, and rate-limit retry.
/// Build the OTel `ResponseFacts` snapshot from a served response. Cost is priced as
/// served by `provider` (matches `track_usage`).
fn response_facts(provider: &str, resp: &ChatResponse) -> sema_otel::ResponseFacts {
    sema_otel::ResponseFacts {
        input_tokens: resp.usage.prompt_tokens,
        output_tokens: resp.usage.completion_tokens,
        cache_read_input_tokens: resp.usage.cache_read_input_tokens,
        cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
        response_model: resp.model.clone(),
        finish_reason: resp.stop_reason.clone(),
        cost_usd: pricing::calculate_cost_for(provider, &resp.usage),
        cache_hit: resp.stop_reason.as_deref() == Some("cache_hit"),
    }
}

/// Flatten chat messages into a `role: content` text blob for opt-in content capture.
fn messages_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content.to_text()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Classify an `LlmError` for the `error.type` span attribute.
fn llm_error_kind(e: &crate::types::LlmError) -> &'static str {
    use crate::types::LlmError::*;
    match e {
        RateLimited { .. } => "rate_limited",
        Api { status, .. } if *status >= 500 => "server_error",
        Api { .. } => "api_error",
        Http(_) => "network_error",
        Parse(_) => "parse_error",
        Config(_) => "config_error",
    }
}

fn do_complete(request: ChatRequest) -> Result<ChatResponse, SemaError> {
    // One CLIENT span per completion. Started here (before cache lookup) so a cache
    // hit still gets a span; request attrs are known up front, provider/model/usage
    // are filled in deeper where they're resolved.
    let span = sema_otel::llm_span("chat");
    span.set_request(
        request.temperature,
        request.max_tokens,
        &request.stop_sequences,
        request.reasoning_effort.as_deref(),
    );
    // Reset the serving-provider stamp so a cache hit (which serves no provider) doesn't
    // inherit a stale name from a prior completion.
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = None);
    let cache_enabled = CACHE_ENABLED.with(|c| c.get());
    if !cache_enabled {
        return do_complete_inner(request, &span);
    }
    // Compute the cache key from the model the request will *logically* use, but
    // without mutating the request that flows into the fallback loop. Pre-filling
    // `request.model` here would make it non-empty and defeat the per-provider
    // default/override substitution in `do_complete_with_provider` — sending the
    // wrong provider's model id down the chain (the original cache+fallback bug).
    let key_model = if request.model.is_empty() {
        primary_model_for_cache()?
    } else {
        request.model.clone()
    };
    let mut key_request = request.clone();
    key_request.model = key_model;
    let cache_key = compute_cache_key(&key_request);
    if let Some(cached) = load_cached(&cache_key) {
        if is_cache_valid(&cached) {
            CACHE_HITS.with(|c| c.set(c.get() + 1));
            // A cache hit makes no provider call: no tokens are consumed and no
            // money is spent. Report ZERO usage so the caller's `track_usage` does
            // not re-charge session cost or burn the budget for a cached response
            // (the provider never saw this request). The cached token counts live
            // in the on-disk/in-memory entry if ever needed; the live accounting
            // must reflect actual spend.
            let resp = ChatResponse {
                content: cached.content,
                role: "assistant".to_string(),
                model: cached.model,
                tool_calls: vec![],
                usage: Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    model: key_request.model.clone(),
                    ..Default::default()
                },
                stop_reason: Some("cache_hit".to_string()),
            };
            // Cache-hit span: no provider served it; tag gen_ai.cache.hit=true with
            // zero usage (matches the zero-usage accounting invariant).
            span.set_dispatch("", &resp.model);
            span.set_response(&response_facts("", &resp));
            return Ok(resp);
        }
    }
    CACHE_MISSES.with(|c| c.set(c.get() + 1));
    let response = do_complete_inner(request, &span)?;
    store_cached(&cache_key, &response);
    Ok(response)
}

/// Resolve the model id used for the cache key when the caller pinned none. With an
/// active fallback chain, the "logical" model is the first chain entry's model (its
/// override if present, else that provider's default); otherwise it's the default
/// provider's default model.
fn primary_model_for_cache() -> Result<String, SemaError> {
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

fn do_complete_inner(
    request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    let fallback_chain = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    match fallback_chain {
        Some(chain) if !chain.is_empty() => {
            let mut last_error = None;
            for entry in &chain {
                match do_complete_with_provider(entry, request.clone(), span) {
                    Ok(resp) => return Ok(resp),
                    Err(e) => {
                        eprintln!(
                            "Provider '{}' failed: {}, trying next...",
                            entry.provider, e
                        );
                        last_error = Some(e);
                    }
                }
            }
            let err = last_error.unwrap_or_else(|| SemaError::Llm("all providers failed".into()));
            span.record_error("provider_error", &err.to_string());
            Err(err)
        }
        _ => {
            let r = do_complete_uncached(request, span);
            if let Err(e) = &r {
                span.record_error("provider_error", &e.to_string());
            }
            r
        }
    }
}

thread_local! {
    /// Base delay for exponential backoff between network retries. Tests set this
    /// to 0 via [`set_retry_base_ms`] so retry behavior is asserted on attempt
    /// count without real sleeps.
    static RETRY_BASE_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(500) };
    /// Max same-provider retries on transient errors (429 / 5xx / network).
    static NETWORK_MAX_RETRIES: std::cell::Cell<u32> = const { std::cell::Cell::new(3) };
}

/// Test hook: set the retry backoff base (ms). 0 disables sleeping.
pub fn set_retry_base_ms(ms: u64) {
    RETRY_BASE_MS.with(|c| c.set(ms));
}

/// Test/config hook: set the max number of same-provider network retries.
pub fn set_network_max_retries(n: u32) {
    NETWORK_MAX_RETRIES.with(|c| c.set(n));
}

/// Whether an `LlmError` is worth retrying on the same provider, and the
/// server-suggested wait in ms. `Some(ms)`: retryable — `ms > 0` honors that wait
/// (429 `retry-after`), `ms == 0` means use computed backoff. `None`: not
/// retryable (4xx non-429, parse/config errors).
fn retryable_wait(err: &crate::types::LlmError) -> Option<u64> {
    use crate::types::LlmError::*;
    match err {
        RateLimited { retry_after_ms } => Some(*retry_after_ms),
        // 5xx are transient server faults; network failures and timeouts surface
        // as Http(_). Both are safe to retry.
        Api { status, .. } if *status >= 500 => Some(0),
        Http(_) => Some(0),
        _ => None,
    }
}

/// Capped exponential backoff with full jitter. A positive server hint wins.
fn retry_backoff_ms(attempt: u32, server_hint: u64) -> u64 {
    const CAP_MS: u64 = 30_000;
    if server_hint > 0 {
        return server_hint.min(CAP_MS);
    }
    let base = RETRY_BASE_MS.with(|c| c.get());
    if base == 0 {
        return 0;
    }
    let ceil = base.saturating_mul(1u64 << attempt.min(6)).min(CAP_MS);
    // Full jitter: a uniform-ish value in [0, ceil]. Sub-nanosecond entropy is
    // plenty here — jitter only affects sleep duration, never control flow.
    let entropy = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    entropy % (ceil + 1)
}

/// Run `provider.complete` with retry on transient errors (429 / 5xx / network),
/// using capped exponential backoff with jitter (429 honors `retry-after`).
fn complete_with_retry(
    provider: &dyn LlmProvider,
    request: &ChatRequest,
    max_retries: u32,
) -> Result<ChatResponse, crate::types::LlmError> {
    let mut attempt = 0u32;
    loop {
        match provider.complete(request.clone()) {
            Ok(resp) => return Ok(resp),
            Err(e) => match retryable_wait(&e) {
                Some(hint) if attempt < max_retries => {
                    let wait = retry_backoff_ms(attempt, hint);
                    // Surface each retry as a child span under the LLM span (the
                    // attempt that triggered the retry + the backoff applied).
                    let rspan = sema_otel::retry_span(attempt + 1);
                    rspan.record_error(llm_error_kind(&e), &e.to_string());
                    rspan.set_wait_ms(wait);
                    drop(rspan);
                    if wait > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(wait));
                    }
                    attempt += 1;
                }
                _ => return Err(e),
            },
        }
    }
}

fn do_complete_with_provider(
    entry: &FallbackEntry,
    mut request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.get(&entry.provider).ok_or_else(|| {
            SemaError::Llm(format!("fallback provider '{}' not found", entry.provider))
        })?;
        // A per-provider chain override wins over any model pinned in the call body
        // (so the chain can target a different model per provider); otherwise fall
        // back to the provider's own default when nothing was pinned. Either way each
        // provider receives a model id valid for itself.
        if let Some(model) = &entry.model {
            request.model = model.clone();
        } else if request.model.is_empty() {
            request.model = provider.default_model().to_string();
        }
        let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
        let resp = complete_with_retry(provider, &request, max_retries)
            .map_err(|e| SemaError::Llm(e.to_string()))?;
        set_serving_provider(&entry.provider);
        // Provider + model + response are all in scope here, before track_usage
        // consumes the serving-provider stamp.
        span.set_dispatch(&entry.provider, &request.model);
        span.set_response(&response_facts(&entry.provider, &resp));
        span.set_messages(
            &messages_text(&request.messages),
            &resp.content,
            request.system.as_deref(),
        );
        Ok(resp)
    })
}

/// Original do_complete logic (provider dispatch + rate-limit retry).
fn do_complete_uncached(
    mut request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    enforce_rate_limit();
    let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
    with_provider(|p| {
        if request.model.is_empty() {
            request.model = p.default_model().to_string();
        }
        let resp = complete_with_retry(p, &request, max_retries)
            .map_err(|e| SemaError::Llm(e.to_string()))?;
        set_serving_provider(p.name());
        // Capture provider/model/response before track_usage consumes the stamp.
        span.set_dispatch(p.name(), &request.model);
        span.set_response(&response_facts(p.name(), &resp));
        span.set_messages(
            &messages_text(&request.messages),
            &resp.content,
            request.system.as_deref(),
        );
        Ok(resp)
    })
}

fn enforce_rate_limit() {
    let rps = RATE_LIMIT_RPS.with(|r| r.get());
    if let Some(rps) = rps {
        let min_interval_ms = (1000.0 / rps) as u64;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = RATE_LIMIT_LAST.with(|l| l.get());
        // saturating_sub: a backward wall-clock adjustment makes `now < last`,
        // which would panic (debug) or wrap to a huge value (release) on plain
        // subtraction. Treat that as "no wait needed". This sleep runs on the
        // synchronous caller thread (the provider's own block_on has already
        // returned), so it does not stall a shared tokio runtime worker.
        let elapsed = now.saturating_sub(last);
        if last > 0 && elapsed < min_interval_ms {
            let sleep_ms = min_interval_ms - elapsed;
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
        }
        let actual_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        RATE_LIMIT_LAST.with(|l| l.set(actual_now));
    }
}

/// Build ToolSchema list from Sema ToolDef values.
fn build_tool_schemas(tools: &[Value]) -> Result<Vec<ToolSchema>, SemaError> {
    let mut schemas = Vec::new();
    for tool in tools {
        let td = tool
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", tool.type_name()))?;
        let params_json = sema_value_to_json_schema(&td.parameters);
        schemas.push(ToolSchema {
            name: td.name.clone(),
            description: td.description.clone(),
            parameters: params_json,
        });
    }
    Ok(schemas)
}

/// Convert a Sema schema map into a JSON Schema object for the LLM API.
fn sema_value_to_json_schema(val: &Value) -> serde_json::Value {
    if let Some(map) = val.as_map_rc() {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (k, v) in map.iter() {
            let key = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            let prop = if let Some(inner) = v.as_map_rc() {
                let mut prop_obj = serde_json::Map::new();
                if let Some(t) = inner.get(&Value::keyword("type")) {
                    let type_str = t
                        .as_keyword()
                        .or_else(|| t.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| "string".to_string());
                    prop_obj.insert("type".to_string(), serde_json::Value::String(type_str));
                }
                if let Some(d) = inner.get(&Value::keyword("description")) {
                    let desc = d
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| d.to_string());
                    prop_obj.insert("description".to_string(), serde_json::Value::String(desc));
                }
                if let Some(e) = inner.get(&Value::keyword("enum")) {
                    if let Some(items) = e.as_seq() {
                        let vals: Vec<serde_json::Value> = items
                            .iter()
                            .map(|v| {
                                serde_json::Value::String(
                                    v.as_str()
                                        .map(|s| s.to_string())
                                        .or_else(|| v.as_keyword())
                                        .unwrap_or_else(|| v.to_string()),
                                )
                            })
                            .collect();
                        prop_obj.insert("enum".to_string(), serde_json::Value::Array(vals));
                    }
                }
                // Mark as required unless :optional #t
                let optional = inner
                    .get(&Value::keyword("optional"))
                    .map(|v| v.is_truthy())
                    .unwrap_or(false);
                if !optional {
                    required.push(serde_json::Value::String(key.clone()));
                }
                serde_json::Value::Object(prop_obj)
            } else {
                required.push(serde_json::Value::String(key.clone()));
                serde_json::json!({"type": "string"})
            };
            properties.insert(key, prop);
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        })
    } else {
        serde_json::json!({"type": "object", "properties": {}})
    }
}

fn sema_list_to_chat_messages(val: &Value) -> Result<Vec<ChatMessage>, SemaError> {
    if val.is_nil() {
        return Ok(Vec::new());
    }
    let items = val
        .as_seq()
        .ok_or_else(|| SemaError::type_error("list of message maps", val.type_name()))?;
    let mut messages = Vec::with_capacity(items.len());
    for item in items.iter() {
        let m = item
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("message map", item.type_name()))?;
        let role = m
            .get(&Value::keyword("role"))
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default();
        let content = m
            .get(&Value::keyword("content"))
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default();
        messages.push(ChatMessage::new(role, content));
    }
    Ok(messages)
}

fn chat_messages_to_sema_list(messages: &[ChatMessage]) -> Value {
    let items: Vec<Value> = messages
        .iter()
        .map(|msg| {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("role"), Value::string(&msg.role));
            map.insert(
                Value::keyword("content"),
                Value::string(&msg.content.to_text()),
            );
            Value::map(map)
        })
        .collect();
    Value::list(items)
}

/// The tool execution loop: send -> check for tool_calls -> execute -> send results -> repeat.
#[allow(clippy::too_many_arguments)]
fn run_tool_loop(
    ctx: &EvalContext,
    initial_messages: Vec<ChatMessage>,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    system: Option<String>,
    reasoning_effort: Option<String>,
    tools: &[Value],
    tool_schemas: &[ToolSchema],
    max_rounds: usize,
    on_tool_call: Option<&Value>,
    agent_name: Option<&str>,
) -> Result<(String, Vec<ChatMessage>), SemaError> {
    // INTERNAL agent span over the whole loop; the per-round `chat` spans (from
    // do_complete) and per-tool spans nest under it via the thread-local stack.
    let _agent_span = sema_otel::agent_span(agent_name);
    let mut messages = initial_messages;
    let mut last_content = String::new();
    // Bound runaway error loops: if the model keeps issuing failing tool calls
    // and never recovers, abort rather than burning every round. Reset on any
    // successful tool call.
    const MAX_CONSECUTIVE_TOOL_ERRORS: usize = 5;
    let mut consecutive_errors: usize = 0;

    for _round in 0..max_rounds {
        let mut request = ChatRequest::new(model.clone(), messages.clone());
        request.max_tokens = max_tokens.or(Some(4096));
        request.temperature = temperature;
        request.system = system.clone();
        request.reasoning_effort = reasoning_effort.clone();
        request.tools = tool_schemas.to_vec();

        let response = do_complete(request)?;
        track_usage(&response.usage)?;
        last_content = response.content.clone();

        if response.tool_calls.is_empty() {
            // Push final assistant message onto history
            if !last_content.is_empty() {
                messages.push(ChatMessage::new("assistant", last_content.clone()));
            }
            return Ok((last_content, messages));
        }

        // Echo the assistant turn that invoked the tools, carrying the tool_calls
        // so the provider can correlate the tool results that follow. This MUST be
        // present (even with empty content) — OpenAI-family providers reject a
        // tool result that isn't preceded by the assistant tool_calls it answers.
        messages.push(ChatMessage::assistant_with_tool_calls(
            response.content.clone(),
            response.tool_calls.clone(),
        ));

        // Execute each tool call and add results
        for tc in &response.tool_calls {
            // Build args map for callback
            let args_value = sema_core::json_to_value(&tc.arguments);

            // Fire "start" event
            if let Some(callback) = on_tool_call {
                let mut event_map = BTreeMap::new();
                event_map.insert(Value::keyword("event"), Value::string("start"));
                event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
                event_map.insert(Value::keyword("args"), args_value.clone());
                let _ = call_value_fn(ctx, callback, &[Value::map(event_map)]);
            }

            let start_time = std::time::Instant::now();
            // INTERNAL tool span (self-times over execute_tool_call, the one real
            // latency source). v1.41 requires the tool name in the span name.
            let tool_desc = tools.iter().find_map(|t| {
                let td = t.as_tool_def_rc()?;
                (td.name == tc.name).then(|| td.description.clone())
            });
            let tspan = sema_otel::tool_span(&tc.name, &tc.id, tool_desc.as_deref());
            // A failing or invalid tool call must NOT abort the whole agent run.
            // Capture the error as the tool result and feed it back so the model
            // can self-correct (bounded by MAX_CONSECUTIVE_TOOL_ERRORS / max_rounds).
            let (result, is_error) = match execute_tool_call(ctx, tools, &tc.name, &tc.arguments) {
                Ok(r) => {
                    consecutive_errors = 0;
                    (r, false)
                }
                Err(e) => {
                    consecutive_errors += 1;
                    (format!("Error: {e}"), true)
                }
            };
            if is_error {
                tspan.record_error("tool_error", &result);
            }
            drop(tspan);
            let duration_ms = start_time.elapsed().as_millis() as i64;

            // Fire "end" event
            if let Some(callback) = on_tool_call {
                let mut event_map = BTreeMap::new();
                event_map.insert(Value::keyword("event"), Value::string("end"));
                event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
                event_map.insert(Value::keyword("args"), args_value);
                // Truncate result for the callback to avoid huge payloads.
                // Use char-boundary truncation: a byte slice (`&result[..200]`)
                // panics when byte 200 lands inside a multi-byte character.
                let result_preview = if result.len() > 200 {
                    format!("{}...", sema_core::truncate_chars(&result, 200))
                } else {
                    result.clone()
                };
                event_map.insert(Value::keyword("result"), Value::string(&result_preview));
                event_map.insert(Value::keyword("error"), Value::bool(is_error));
                event_map.insert(Value::keyword("duration-ms"), Value::int(duration_ms));
                let _ = call_value_fn(ctx, callback, &[Value::map(event_map)]);
            }

            // Correlated tool result — keyed by the call id and tool name — rather
            // than free-form user text, so every provider can match it to the call.
            messages.push(ChatMessage::tool_result(
                tc.id.clone(),
                tc.name.clone(),
                result,
            ));

            if consecutive_errors >= MAX_CONSECUTIVE_TOOL_ERRORS {
                return Err(SemaError::Llm(format!(
                    "aborting agent run after {consecutive_errors} consecutive tool errors"
                )));
            }
        }
    }

    // Push final assistant message if we exhausted rounds
    if !last_content.is_empty() {
        messages.push(ChatMessage::new("assistant", last_content.clone()));
    }
    Ok((last_content, messages))
}

/// Execute a tool call by finding the handler and invoking it.
fn execute_tool_call(
    ctx: &EvalContext,
    tools: &[Value],
    name: &str,
    arguments: &serde_json::Value,
) -> Result<String, SemaError> {
    // Find the tool definition
    let tool_def = tools
        .iter()
        .find_map(|t| {
            let td = t.as_tool_def_rc()?;
            if td.name == name {
                Some(td)
            } else {
                None
            }
        })
        .ok_or_else(|| SemaError::Llm(format!("tool not found: {name}")))?;

    // Validate the model-supplied arguments against the tool's parameter schema
    // before invoking the handler, so a missing/wrong-typed argument is reported
    // back to the model (via the loop's error-recovery path) and it can retry with
    // corrected args — rather than silently calling the handler with bad input.
    // (Reuses the extraction validator; both schema and args use keyword keys.)
    let args_map = sema_core::json_to_value(arguments);
    if let Err(msg) = validate_extraction(&args_map, &tool_def.parameters) {
        return Err(SemaError::Llm(format!(
            "invalid arguments for tool '{name}': {msg}"
        )));
    }

    // Convert JSON arguments to Sema values and call the handler
    let sema_args = json_args_to_sema(&tool_def.parameters, arguments, &tool_def.handler);
    let result = call_value_fn(ctx, &tool_def.handler, &sema_args)?;

    // Convert result to string for sending back to LLM
    if let Some(s) = result.as_str() {
        return Ok(s.to_string());
    }
    if result.as_map_rc().is_some() || result.as_seq().is_some() {
        // JSON-encode complex results
        let json = sema_core::value_to_json_lossy(&result);
        Ok(serde_json::to_string(&json).unwrap_or_else(|_| result.to_string()))
    } else {
        Ok(result.to_string())
    }
}

/// Convert JSON arguments into a list of Sema values based on the parameter schema order.
/// When the handler is a lambda, uses its param names (declaration order) instead of
/// BTreeMap key order (alphabetical), fixing argument ordering mismatches.
fn json_args_to_sema(params: &Value, arguments: &serde_json::Value, handler: &Value) -> Vec<Value> {
    if let serde_json::Value::Object(json_obj) = arguments {
        // Prefer lambda param names (preserves declaration order) over BTreeMap keys
        if let Some(lambda) = handler.as_lambda_rc() {
            return lambda
                .params
                .iter()
                .map(|name| {
                    json_obj
                        .get(&resolve(*name))
                        .map(sema_core::json_to_value)
                        .unwrap_or(Value::nil())
                })
                .collect();
        }
        // Fallback: use param map keys (BTreeMap order — alphabetical)
        if let Some(param_map) = params.as_map_rc() {
            return param_map
                .keys()
                .map(|k| {
                    let key_str = k
                        .as_keyword()
                        .or_else(|| k.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| k.to_string());
                    json_obj
                        .get(&key_str)
                        .map(sema_core::json_to_value)
                        .unwrap_or(Value::nil())
                })
                .collect();
        }
    }
    vec![sema_core::json_to_value(arguments)]
}

/// Call a Sema value as a function (lambda or native).
fn call_value_fn(ctx: &EvalContext, func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    if let Some(native) = func.as_native_fn_rc() {
        return (native.func)(ctx, args);
    }
    if let Some(lambda) = func.as_lambda_rc() {
        let env = Env::with_parent(Rc::new(lambda.env.clone()));
        // Bind params
        if let Some(ref rest) = lambda.rest_param {
            if args.len() < lambda.params.len() {
                return Err(SemaError::arity(
                    lambda
                        .name
                        .map(resolve)
                        .unwrap_or_else(|| "lambda".to_string()),
                    format!("{}+", lambda.params.len()),
                    args.len(),
                ));
            }
            for (param, arg) in lambda.params.iter().zip(args.iter()) {
                env.set(*param, arg.clone());
            }
            let rest_args = args[lambda.params.len()..].to_vec();
            env.set(*rest, Value::list(rest_args));
        } else {
            if args.len() != lambda.params.len() {
                return Err(SemaError::arity(
                    lambda
                        .name
                        .map(resolve)
                        .unwrap_or_else(|| "lambda".to_string()),
                    lambda.params.len().to_string(),
                    args.len(),
                ));
            }
            for (param, arg) in lambda.params.iter().zip(args.iter()) {
                env.set(*param, arg.clone());
            }
        }
        // Self-reference
        if let Some(name) = lambda.name {
            env.set(
                name,
                Value::lambda(sema_core::Lambda {
                    params: lambda.params.clone(),
                    rest_param: lambda.rest_param,
                    body: lambda.body.clone(),
                    env: lambda.env.clone(),
                    name: lambda.name,
                }),
            );
        }
        // Eval body using the full evaluator (supports let, if, cond, etc.)
        let mut result = Value::nil();
        for expr in &lambda.body {
            result = full_eval(ctx, expr, &env)?;
        }
        return Ok(result);
    }
    Err(SemaError::eval(format!(
        "not callable: {} ({})",
        func,
        func.type_name()
    )))
}

/// Minimal evaluator for use within LLM builtins (avoids circular dep with sema-eval).
fn simple_eval(ctx: &EvalContext, expr: &Value, env: &Env) -> Result<Value, SemaError> {
    match expr.view() {
        ValueView::Symbol(spur) => env
            .get(spur)
            .ok_or_else(|| SemaError::Unbound(resolve(spur))),
        ValueView::List(items) if !items.is_empty() => {
            let func_val = simple_eval(ctx, &items[0], env)?;
            let mut args = Vec::new();
            for arg in &items[1..] {
                args.push(simple_eval(ctx, arg, env)?);
            }
            call_value_fn(ctx, &func_val, &args)
        }
        _ => Ok(expr.clone()),
    }
}

/// Detect media type from file magic bytes.
fn detect_media_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"%PDF") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{intern, Lambda};
    use serde_json::json;

    #[test]
    fn enforce_rate_limit_survives_backward_clock() {
        // A last-request timestamp in the future (wall clock jumped backward)
        // must not panic on the `now - last` subtraction (debug overflow check)
        // and must not produce a huge sleep.
        RATE_LIMIT_RPS.with(|r| r.set(Some(10.0)));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        RATE_LIMIT_LAST.with(|l| l.set(now + 1_000_000));
        let start = std::time::Instant::now();
        enforce_rate_limit();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "backward clock should not cause a long sleep"
        );
        RATE_LIMIT_RPS.with(|r| r.set(None));
        RATE_LIMIT_LAST.with(|l| l.set(0));
    }

    #[test]
    fn url_host_extraction() {
        assert_eq!(
            url_host("https://api.openai.com/v1").as_deref(),
            Some("api.openai.com")
        );
        assert_eq!(
            url_host("http://localhost:11434").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            url_host("http://user:pass@10.0.0.1:8080/x").as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(url_host("http://[::1]:9200/").as_deref(), Some("::1"));
        assert_eq!(
            url_host("http://169.254.169.254/latest").as_deref(),
            Some("169.254.169.254")
        );
    }

    #[test]
    fn internal_hosts_are_flagged() {
        for h in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1", // ipv4-mapped loopback
        ] {
            assert!(is_internal_host(h), "{h} should be internal");
        }
    }

    #[test]
    fn public_hosts_are_allowed() {
        for h in ["api.openai.com", "api.anthropic.com", "8.8.8.8", "1.1.1.1"] {
            assert!(!is_internal_host(h), "{h} should be allowed");
        }
    }

    #[test]
    fn internal_hosts_flagged_through_inet_aton_encodings() {
        // getaddrinfo accepts these and resolves them to loopback/internal,
        // but Ipv4Addr::from_str rejects them — the SSRF bypass.
        for h in [
            "2130706433", // decimal 127.0.0.1
            "0177.0.0.1", // octal first octet
            "0x7f.0.0.1", // hex first octet
            "0x7f000001", // single hex 32-bit 127.0.0.1
            "127.1",      // short form -> 127.0.0.1
            "127.0.0.1.", // trailing dot
            "0xA9FEA9FE", // 169.254.169.254 cloud metadata
        ] {
            assert!(is_internal_host(h), "{h} should be flagged internal");
        }
    }

    #[test]
    fn public_numeric_encodings_still_allowed() {
        // Numeric forms that decode to genuinely public addresses must not be
        // over-blocked (don't break legit numeric base-urls).
        for h in [
            "134744072",  // decimal 8.8.8.8
            "0x08080808", // hex 8.8.8.8
            "8.8.8.8.",   // trailing dot, public
            "010.0.0.1",  // octal 8.0.0.1 -> public
        ] {
            assert!(!is_internal_host(h), "{h} should be allowed (public)");
        }
    }

    #[test]
    fn guard_blocks_internal_only_when_sandboxed() {
        let mut opts = BTreeMap::new();
        opts.insert(
            Value::keyword("base-url"),
            Value::string("http://169.254.169.254/"),
        );
        // Unrestricted (normal CLI/REPL): allowed — local proxies / Ollama work.
        assert!(guard_provider_url(true, &opts).is_ok());
        // Sandboxed: rejected.
        assert!(guard_provider_url(false, &opts).is_err());

        let mut public_opts = BTreeMap::new();
        public_opts.insert(
            Value::keyword("base-url"),
            Value::string("https://api.openai.com/v1"),
        );
        assert!(guard_provider_url(false, &public_opts).is_ok());
    }

    fn make_lambda(params: &[&str]) -> Value {
        Value::lambda(Lambda {
            params: params.iter().map(|s| intern(s)).collect(),
            rest_param: None,
            body: vec![Value::nil()],
            env: Env::new(),
            name: None,
        })
    }

    fn make_param_map(keys: &[&str]) -> Value {
        let mut map = BTreeMap::new();
        for k in keys {
            map.insert(Value::keyword(k), Value::map(BTreeMap::new()));
        }
        Value::map(map)
    }

    // -- json_args_to_sema tests --

    #[test]
    fn test_json_args_to_sema_lambda_declaration_order() {
        // Params declared as (path, content) — but alphabetically content < path.
        // The lambda path must use declaration order, not alphabetical.
        let handler = make_lambda(&["path", "content"]);
        let params = make_param_map(&["path", "content"]);
        let args = json!({"path": "/tmp/test.txt", "content": "hello world"});

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], Value::string("/tmp/test.txt"));
        assert_eq!(result[1], Value::string("hello world"));
    }

    #[test]
    fn test_json_args_to_sema_many_params_declaration_order() {
        // 4 params where alphabetical (a, b, c, d) != declaration order (d, b, a, c)
        let handler = make_lambda(&["delta", "bravo", "alpha", "charlie"]);
        let params = make_param_map(&["delta", "bravo", "alpha", "charlie"]);
        let args = json!({
            "alpha": "A",
            "bravo": "B",
            "charlie": "C",
            "delta": "D"
        });

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0], Value::string("D")); // delta first (declaration order)
        assert_eq!(result[1], Value::string("B")); // bravo second
        assert_eq!(result[2], Value::string("A")); // alpha third
        assert_eq!(result[3], Value::string("C")); // charlie fourth
    }

    #[test]
    fn test_json_args_to_sema_missing_arg_yields_nil() {
        let handler = make_lambda(&["path", "content"]);
        let params = make_param_map(&["path", "content"]);
        let args = json!({"path": "/tmp/test.txt"});

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], Value::string("/tmp/test.txt"));
        assert_eq!(result[1], Value::nil());
    }

    #[test]
    fn test_json_args_to_sema_non_lambda_falls_back_to_btreemap() {
        // With a NativeFn handler, should fall back to param_map key order (alphabetical).
        let handler = Value::native_fn(NativeFn::simple("test", |_args| Ok(Value::nil())));
        let params = make_param_map(&["zebra", "apple"]);
        let args = json!({"zebra": "Z", "apple": "A"});

        let result = json_args_to_sema(&params, &args, &handler);

        // BTreeMap sorts alphabetically: apple < zebra
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], Value::string("A")); // apple first (alphabetical)
        assert_eq!(result[1], Value::string("Z")); // zebra second
    }

    #[test]
    fn test_json_args_to_sema_non_object_json() {
        let handler = make_lambda(&["x"]);
        let params = make_param_map(&["x"]);
        let args = json!("just a string");

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], Value::string("just a string"));
    }

    #[test]
    fn test_json_args_to_sema_mixed_types() {
        let handler = make_lambda(&["name", "age", "active"]);
        let params = make_param_map(&["name", "age", "active"]);
        let args = json!({"name": "Alice", "age": 30, "active": true});

        let result = json_args_to_sema(&params, &args, &handler);

        // Declaration order: name, age, active
        assert_eq!(result[0], Value::string("Alice"));
        assert_eq!(result[1], Value::int(30));
        assert_eq!(result[2], Value::bool(true));
    }

    // -- execute_tool_call tests --

    #[test]
    fn test_execute_tool_call_arg_ordering() {
        // Tool with params where alphabetical != declaration order.
        // Handler returns "path={path}, content={content}" to verify ordering.
        let handler = Value::lambda(Lambda {
            params: vec![intern("path"), intern("content")],
            rest_param: None,
            body: vec![
                // Body: (string-append path "|" content)
                // We can't call string-append without a full evaluator,
                // so just return the first param to verify it's the path.
                Value::symbol("path"),
            ],
            env: Env::new(),
            name: Some(intern("write-file-handler")),
        });

        let tool = Value::tool_def(sema_core::ToolDefinition {
            name: "write-file".to_string(),
            description: "Write content to a file".to_string(),
            parameters: make_param_map(&["path", "content"]),
            handler,
        });

        let args = json!({"path": "/tmp/test.txt", "content": "file body here"});
        let ctx = EvalContext::new();
        let result = execute_tool_call(&ctx, &[tool], "write-file", &args).unwrap();

        // If ordering is wrong (alphabetical), content would be bound to `path`
        // and we'd get "file body here" instead of the actual path.
        assert_eq!(result, "/tmp/test.txt");
    }

    #[test]
    fn test_execute_tool_call_reverse_alpha_order() {
        // Declare params (z_last, a_first) — exact reverse of alphabetical.
        let handler = Value::lambda(Lambda {
            params: vec![intern("z_last"), intern("a_first")],
            rest_param: None,
            body: vec![Value::symbol("z_last")],
            env: Env::new(),
            name: Some(intern("test-handler")),
        });

        let tool = Value::tool_def(sema_core::ToolDefinition {
            name: "test-tool".to_string(),
            description: "test".to_string(),
            parameters: make_param_map(&["z_last", "a_first"]),
            handler,
        });

        let args = json!({"z_last": "ZLAST", "a_first": "AFIRST"});
        let ctx = EvalContext::new();
        let result = execute_tool_call(&ctx, &[tool], "test-tool", &args).unwrap();

        // Lambda body returns z_last — must be "ZLAST", not "AFIRST"
        assert_eq!(result, "ZLAST");
    }

    #[test]
    fn test_validate_extraction_missing_key() {
        let schema = {
            let mut map = BTreeMap::new();
            let mut name_spec = BTreeMap::new();
            name_spec.insert(Value::keyword("type"), Value::keyword("string"));
            map.insert(Value::keyword("name"), Value::map(name_spec));
            let mut age_spec = BTreeMap::new();
            age_spec.insert(Value::keyword("type"), Value::keyword("number"));
            map.insert(Value::keyword("age"), Value::map(age_spec));
            Value::map(map)
        };
        let result = {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("name"), Value::string("Alice"));
            Value::map(map)
        };
        let err = validate_extraction(&result, &schema).unwrap_err();
        assert!(err.contains("missing key: age"), "got: {err}");
    }

    #[test]
    fn test_validate_extraction_wrong_type() {
        let schema = {
            let mut map = BTreeMap::new();
            let mut name_spec = BTreeMap::new();
            name_spec.insert(Value::keyword("type"), Value::keyword("string"));
            map.insert(Value::keyword("name"), Value::map(name_spec));
            Value::map(map)
        };
        let result = {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("name"), Value::int(42));
            Value::map(map)
        };
        let err = validate_extraction(&result, &schema).unwrap_err();
        assert!(err.contains("expected string"), "got: {err}");
    }

    #[test]
    fn test_validate_extraction_valid() {
        let schema = {
            let mut map = BTreeMap::new();
            let mut name_spec = BTreeMap::new();
            name_spec.insert(Value::keyword("type"), Value::keyword("string"));
            map.insert(Value::keyword("name"), Value::map(name_spec));
            Value::map(map)
        };
        let result = {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("name"), Value::string("Alice"));
            Value::map(map)
        };
        assert!(validate_extraction(&result, &schema).is_ok());
    }

    #[test]
    fn test_format_reask_prompt() {
        let prev_response = r#"{"name": 42}"#;
        let errors = "key name: expected string, got integer";
        let schema_desc = r#"{ "name": <string> }"#;
        let result = format_reask_prompt(prev_response, errors, schema_desc);
        assert!(result.contains("Previous response:"));
        assert!(result.contains(prev_response));
        assert!(result.contains(errors));
    }

    #[test]
    fn test_fallback_chain_thread_local() {
        FALLBACK_CHAIN.with(|chain| {
            assert!(chain.borrow().is_none());
            *chain.borrow_mut() = Some(vec![
                FallbackEntry {
                    provider: "openai".to_string(),
                    model: None,
                },
                FallbackEntry {
                    provider: "anthropic".to_string(),
                    model: None,
                },
            ]);
            assert_eq!(chain.borrow().as_ref().unwrap().len(), 2);
            *chain.borrow_mut() = None;
        });
    }

    #[test]
    fn test_parse_fallback_entry_bare_keyword() {
        let entry = parse_fallback_entry(&Value::keyword("anthropic")).unwrap();
        assert_eq!(entry.provider, "anthropic");
        assert_eq!(entry.model, None);
    }

    #[test]
    fn test_parse_fallback_entry_pair() {
        let v = Value::vector(vec![Value::keyword("openai"), Value::string("gpt-5.5")]);
        let entry = parse_fallback_entry(&v).unwrap();
        assert_eq!(entry.provider, "openai");
        assert_eq!(entry.model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn test_parse_fallback_entry_map() {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("provider"), Value::keyword("anthropic"));
        map.insert(Value::keyword("model"), Value::string("claude-opus-4-8"));
        let entry = parse_fallback_entry(&Value::map(map)).unwrap();
        assert_eq!(entry.provider, "anthropic");
        assert_eq!(entry.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn test_parse_fallback_entry_bad_pair_len() {
        let v = Value::vector(vec![
            Value::keyword("openai"),
            Value::string("a"),
            Value::string("b"),
        ]);
        assert!(parse_fallback_entry(&v).is_err());
    }
}
