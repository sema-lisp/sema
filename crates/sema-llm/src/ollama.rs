use crate::provider::LlmProvider;
use crate::types::{ChatRequest, ChatResponse, LlmError, ToolCall, ToolSchema, Usage};

/// Convert our `ToolSchema` list into Ollama's (OpenAI-compatible) tools JSON.
fn build_tools_json(tools: &[ToolSchema]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    serde_json::Value::Array(arr)
}

/// Parse tool calls from an Ollama response JSON value.
/// Ollama returns `message.tool_calls` as an array of `{ function: { name, arguments } }`.
/// We generate synthetic IDs since Ollama doesn't provide them.
fn parse_tool_calls(message: &serde_json::Value) -> Vec<ToolCall> {
    let Some(arr) = message.get("tool_calls").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .filter_map(|(i, tc)| {
            let func = tc.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let arguments = func
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            Some(ToolCall {
                id: format!("ollama-call-{i}"),
                name,
                arguments,
                thought_signature: None,
            })
        })
        .collect()
}

/// Build the Ollama `messages` array, handling assistant tool_calls and
/// correlated tool results (Ollama's chat API is OpenAI-shaped).
fn build_ollama_messages(request: &ChatRequest) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    if let Some(ref system) = request.system {
        messages.push(serde_json::json!({ "role": "system", "content": system }));
    }
    for msg in &request.messages {
        if !msg.tool_calls.is_empty() {
            // Assistant turn that invoked tools (Ollama takes arguments as an object).
            let tcs: Vec<serde_json::Value> = msg
                .tool_calls
                .iter()
                .map(|tc| serde_json::json!({ "function": { "name": tc.name, "arguments": tc.arguments } }))
                .collect();
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": msg.content.to_text(),
                "tool_calls": tcs,
            }));
            continue;
        }
        if msg.role == "tool" {
            let mut m = serde_json::json!({ "role": "tool", "content": msg.content.to_text() });
            if let Some(name) = &msg.tool_name {
                m["tool_name"] = serde_json::json!(name);
            }
            messages.push(m);
            continue;
        }
        let mut m = serde_json::json!({ "role": msg.role, "content": msg.content.to_text() });
        if let crate::types::MessageContent::Blocks(blocks) = &msg.content {
            let images: Vec<&str> = blocks
                .iter()
                .filter_map(|b| {
                    if let crate::types::ContentBlock::Image { data, .. } = b {
                        Some(data.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if !images.is_empty() {
                m["images"] = serde_json::json!(images);
            }
        }
        messages.push(m);
    }
    messages
}

pub struct OllamaProvider {
    host: String,
    default_model: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(host: Option<String>, default_model: Option<String>) -> Result<Self, LlmError> {
        let client = crate::http::create_client(None)?;
        Ok(OllamaProvider {
            host: host.unwrap_or_else(|| {
                std::env::var("OLLAMA_HOST")
                    .unwrap_or_else(|_| "http://localhost:11434".to_string())
            }),
            default_model: default_model.unwrap_or_else(|| "gemma4".to_string()),
            client,
        })
    }

    fn resolve_model(&self, model: &str) -> String {
        if model.is_empty() {
            self.default_model.clone()
        } else {
            model.to_string()
        }
    }

    async fn complete_async(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let model = self.resolve_model(&request.model);
        let url = format!("{}/api/chat", self.host);

        let messages = build_ollama_messages(&request);

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });

        // Add tools if provided
        if !request.tools.is_empty() {
            body["tools"] = build_tools_json(&request.tools);
        }

        // Options
        let mut options = serde_json::Map::new();
        if let Some(max_tokens) = request.max_tokens {
            options.insert("num_predict".to_string(), serde_json::json!(max_tokens));
        }
        if let Some(temp) = request.temperature {
            options.insert("temperature".to_string(), serde_json::json!(temp));
        }
        if !options.is_empty() {
            body["options"] = serde_json::Value::Object(options);
        }

        let resp = self
            .client
            .post(&url)
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

        let content = api_resp
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let tool_calls = api_resp
            .get("message")
            .map(parse_tool_calls)
            .unwrap_or_default();

        let stop_reason = if tool_calls.is_empty() {
            "stop"
        } else {
            "tool_use"
        };

        let prompt_tokens = api_resp
            .get("prompt_eval_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let completion_tokens = api_resp
            .get("eval_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        Ok(ChatResponse {
            content,
            role: "assistant".to_string(),
            model: model.clone(),
            tool_calls,
            usage: Usage {
                prompt_tokens,
                completion_tokens,
                model,
                ..Default::default()
            },
            stop_reason: Some(stop_reason.to_string()),
        })
    }

    async fn stream_complete_async(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        let model = self.resolve_model(&request.model);
        let url = format!("{}/api/chat", self.host);

        let messages = build_ollama_messages(&request);

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });

        // Add tools if provided
        if !request.tools.is_empty() {
            body["tools"] = build_tools_json(&request.tools);
        }

        let mut options = serde_json::Map::new();
        if let Some(max_tokens) = request.max_tokens {
            options.insert("num_predict".to_string(), serde_json::json!(max_tokens));
        }
        if let Some(temp) = request.temperature {
            options.insert("temperature".to_string(), serde_json::json!(temp));
        }
        if !options.is_empty() {
            body["options"] = serde_json::Value::Object(options);
        }

        let resp = self
            .client
            .post(&url)
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

        let mut full_content = String::new();
        let mut prompt_tokens = 0u32;
        let mut completion_tokens = 0u32;
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        crate::ndjson::parse_ndjson_stream(resp, |json| {
            // Extract content delta
            if let Some(content) = json.pointer("/message/content").and_then(|v| v.as_str()) {
                if !content.is_empty() {
                    full_content.push_str(content);
                    on_chunk(content)?;
                }
            }

            // Check if done — final chunk has usage info and tool calls
            if let Some(true) = json.get("done").and_then(|v| v.as_bool()) {
                prompt_tokens = json
                    .get("prompt_eval_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                completion_tokens =
                    json.get("eval_count").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

                // Tool calls appear in the final message
                if let Some(msg) = json.get("message") {
                    tool_calls = parse_tool_calls(msg);
                }
            }
            Ok(())
        })
        .await?;

        let stop_reason = if tool_calls.is_empty() {
            "stop"
        } else {
            "tool_use"
        };

        Ok(ChatResponse {
            content: full_content,
            role: "assistant".to_string(),
            model: model.clone(),
            tool_calls,
            usage: Usage {
                prompt_tokens,
                completion_tokens,
                model,
                ..Default::default()
            },
            stop_reason: Some(stop_reason.to_string()),
        })
    }
}

impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        sema_io::io_block_on(self.complete_async(request))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn complete_future(
        &self,
        request: ChatRequest,
    ) -> Option<crate::provider::BoxCompletionFuture<'_>> {
        Some(Box::pin(self.complete_async(request)))
    }

    fn stream_complete(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        // io_block_on drives ON THIS thread: `on_chunk` may touch non-Send Sema
        // values and must never migrate to a pool worker.
        sema_io::io_block_on(self.stream_complete_async(request, on_chunk))
    }

    fn batch_complete(&self, requests: Vec<ChatRequest>) -> Vec<Result<ChatResponse, LlmError>> {
        sema_io::io_block_on(async {
            let futures: Vec<_> = requests
                .into_iter()
                .map(|req| self.complete_async(req))
                .collect();
            futures::future::join_all(futures).await
        })
    }
}
