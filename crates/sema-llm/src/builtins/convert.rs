use super::*;

pub(super) fn message_to_chat_message(m: &Message) -> ChatMessage {
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

pub(super) fn extract_messages(val: &Value) -> Result<Vec<ChatMessage>, SemaError> {
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

pub(super) fn sema_list_to_chat_messages(val: &Value) -> Result<Vec<ChatMessage>, SemaError> {
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
        let mut msg = ChatMessage::new(role, content);
        // Restore tool-call correlation written by chat_messages_to_sema_list so a
        // re-sent history keeps the assistant tool_calls and the tool-result ids.
        if let Some(tcs) = m
            .get(&Value::keyword("tool-calls"))
            .and_then(|v| v.as_seq())
        {
            msg.tool_calls = tcs
                .iter()
                .filter_map(|tc| {
                    let tm = tc.as_map_rc()?;
                    Some(ToolCall {
                        id: tm
                            .get(&Value::keyword("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: tm
                            .get(&Value::keyword("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        arguments: tm
                            .get(&Value::keyword("arguments"))
                            .map(sema_core::value_to_json_lossy)
                            .unwrap_or_else(|| serde_json::json!({})),
                        thought_signature: tm
                            .get(&Value::keyword("thought-signature"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect();
        }
        msg.tool_call_id = m
            .get(&Value::keyword("tool-call-id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        msg.tool_name = m
            .get(&Value::keyword("tool-name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        messages.push(msg);
    }
    Ok(messages)
}

pub(super) fn chat_messages_to_sema_list(messages: &[ChatMessage]) -> Value {
    let items: Vec<Value> = messages
        .iter()
        .map(|msg| {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("role"), Value::string(&msg.role));
            map.insert(
                Value::keyword("content"),
                Value::string(&msg.content.to_text()),
            );
            // Preserve tool-call correlation so this history re-sends validly on the
            // next turn. Without it, a re-sent assistant tool-call turn loses its
            // tool_calls and the tool result loses its id — providers 400 on the
            // empty tool_use_id / tool_call_id.
            if !msg.tool_calls.is_empty() {
                let tcs: Vec<Value> = msg
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        let mut m = BTreeMap::new();
                        m.insert(Value::keyword("id"), Value::string(&tc.id));
                        m.insert(Value::keyword("name"), Value::string(&tc.name));
                        m.insert(
                            Value::keyword("arguments"),
                            sema_core::json_to_value(&tc.arguments),
                        );
                        // Gemini's opaque thoughtSignature must survive the Sema
                        // round-trip too, or a :messages/:session continuation
                        // re-sends the turn without it and Gemini 400s.
                        if let Some(ref sig) = tc.thought_signature {
                            m.insert(Value::keyword("thought-signature"), Value::string(sig));
                        }
                        Value::map(m)
                    })
                    .collect();
                map.insert(Value::keyword("tool-calls"), Value::list(tcs));
            }
            if let Some(ref id) = msg.tool_call_id {
                map.insert(Value::keyword("tool-call-id"), Value::string(id));
            }
            if let Some(ref name) = msg.tool_name {
                map.insert(Value::keyword("tool-name"), Value::string(name));
            }
            Value::map(map)
        })
        .collect();
    Value::list(items)
}

/// Detect media type from file magic bytes.
pub(super) fn detect_media_type(bytes: &[u8]) -> &'static str {
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
