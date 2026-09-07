use super::*;

#[derive(Clone, Copy)]
pub(super) enum ConversationCallbackKind {
    Filter,
    Map,
    MapRole,
    Find,
}

impl ConversationCallbackKind {
    fn name(self) -> &'static str {
        match self {
            Self::Filter => "conversation/filter",
            Self::Map => "conversation/map",
            Self::MapRole => "conversation/map-role",
            Self::Find => "conversation/find",
        }
    }

    fn arity(self) -> usize {
        match self {
            Self::MapRole => 3,
            Self::Filter | Self::Map | Self::Find => 2,
        }
    }
}

pub(super) enum ConversationCallbackOperation {
    Filter,
    Map,
    MapRole(Role),
    Find,
}

pub(super) struct ConversationCallbackPlan {
    pub(super) conversation: Rc<Conversation>,
    pub(super) callback: Value,
    pub(super) operation: ConversationCallbackOperation,
}

pub(super) fn prepare_conversation_callback(
    kind: ConversationCallbackKind,
    args: &[Value],
) -> Result<ConversationCallbackPlan, SemaError> {
    let expected = kind.arity();
    if args.len() != expected {
        return Err(SemaError::arity(
            kind.name(),
            expected.to_string(),
            args.len(),
        ));
    }
    let conversation = args[0]
        .as_conversation_rc()
        .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
    let (callback, operation) = match kind {
        ConversationCallbackKind::Filter => {
            (args[1].clone(), ConversationCallbackOperation::Filter)
        }
        ConversationCallbackKind::Map => (args[1].clone(), ConversationCallbackOperation::Map),
        ConversationCallbackKind::MapRole => (
            args[2].clone(),
            ConversationCallbackOperation::MapRole(parse_role(&args[1], kind.name())?),
        ),
        ConversationCallbackKind::Find => (args[1].clone(), ConversationCallbackOperation::Find),
    };
    Ok(ConversationCallbackPlan {
        conversation,
        callback,
        operation,
    })
}

pub(super) fn run_conversation_callback_sync(
    ctx: &EvalContext,
    plan: ConversationCallbackPlan,
) -> Result<Value, SemaError> {
    match plan.operation {
        ConversationCallbackOperation::Filter => {
            let mut messages = Vec::new();
            for message in &plan.conversation.messages {
                let result = sema_core::call_callback(
                    ctx,
                    &plan.callback,
                    &[Value::message(message.clone())],
                )?;
                if result.is_truthy() {
                    messages.push(message.clone());
                }
            }
            Ok(conversation_with_messages(&plan.conversation, messages))
        }
        ConversationCallbackOperation::Map => {
            let mut results = Vec::with_capacity(plan.conversation.messages.len());
            for message in &plan.conversation.messages {
                results.push(sema_core::call_callback(
                    ctx,
                    &plan.callback,
                    &[Value::message(message.clone())],
                )?);
            }
            Ok(Value::list(results))
        }
        ConversationCallbackOperation::MapRole(role) => {
            let mut messages = Vec::with_capacity(plan.conversation.messages.len());
            for message in &plan.conversation.messages {
                if message.role == role {
                    let result = sema_core::call_callback(
                        ctx,
                        &plan.callback,
                        &[Value::message(message.clone())],
                    )?;
                    let transformed = result
                        .as_message_rc()
                        .ok_or_else(|| SemaError::type_error("message", result.type_name()))?;
                    messages.push((*transformed).clone());
                } else {
                    messages.push(message.clone());
                }
            }
            Ok(conversation_with_messages(&plan.conversation, messages))
        }
        ConversationCallbackOperation::Find => {
            for message in &plan.conversation.messages {
                let argument = Value::message(message.clone());
                if sema_core::call_callback(ctx, &plan.callback, std::slice::from_ref(&argument))?
                    .is_truthy()
                {
                    return Ok(argument);
                }
            }
            Ok(Value::nil())
        }
    }
}

pub(super) fn conversation_with_messages(
    conversation: &Conversation,
    messages: Vec<Message>,
) -> Value {
    Value::conversation(Conversation {
        messages,
        model: conversation.model.clone(),
        metadata: conversation.metadata.clone(),
    })
}

pub(super) fn register_conversation_callback_fn(env: &Env, kind: ConversationCallbackKind) {
    let name = kind.name();
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx_runtime(
            name,
            move |ctx, args| {
                run_conversation_callback_sync(ctx, prepare_conversation_callback(kind, args)?)
            },
            move |_native_ctx, args| {
                ConversationCallbackDriver::start(prepare_conversation_callback(kind, args)?)
            },
        )),
    );
}

pub(super) struct ConversationCallbackDriver {
    pub(super) plan: ConversationCallbackPlan,
    pub(super) next_message: usize,
    pub(super) active_message: Option<usize>,
    pub(super) active_argument: Option<Value>,
    pub(super) values: Vec<Value>,
    pub(super) messages: Vec<Message>,
}

impl sema_core::runtime::Trace for ConversationCallbackDriver {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.plan.callback));
        if let Some(argument) = &self.active_argument {
            sink(sema_core::cycle::GcEdge::Value(argument));
        }
        for value in &self.values {
            sink(sema_core::cycle::GcEdge::Value(value));
        }
        true
    }
}

impl ConversationCallbackDriver {
    fn start(plan: ConversationCallbackPlan) -> sema_core::runtime::NativeResult {
        Box::new(Self {
            plan,
            next_message: 0,
            active_message: None,
            active_argument: None,
            values: Vec::new(),
            messages: Vec::new(),
        })
        .advance()
    }

    fn advance(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome};

        while let Some(message) = self.plan.conversation.messages.get(self.next_message) {
            let index = self.next_message;
            self.next_message += 1;
            if let ConversationCallbackOperation::MapRole(role) = &self.plan.operation {
                if message.role != *role {
                    self.messages.push(message.clone());
                    continue;
                }
            }
            self.active_message = Some(index);
            let argument = Value::message(message.clone());
            self.active_argument = Some(argument.clone());
            return Ok(NativeOutcome::Call(NativeCall {
                callable: self.plan.callback.clone(),
                args: vec![argument],
                continuation: self,
            }));
        }

        let Self {
            plan,
            values,
            messages,
            ..
        } = *self;
        let value = match plan.operation {
            ConversationCallbackOperation::Filter | ConversationCallbackOperation::MapRole(_) => {
                conversation_with_messages(&plan.conversation, messages)
            }
            ConversationCallbackOperation::Map => Value::list(values),
            ConversationCallbackOperation::Find => Value::nil(),
        };
        Ok(NativeOutcome::Return(value))
    }
}

impl sema_core::runtime::NativeContinuation for ConversationCallbackDriver {
    fn resume(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput};

        let Some((index, argument)) = self.active_message.take().zip(self.active_argument.take())
        else {
            return Err(SemaError::eval(format!(
                "{} callback resumed without an active message",
                self.plan.operation.name()
            )));
        };
        match input {
            ResumeInput::Returned(value) => match &self.plan.operation {
                ConversationCallbackOperation::Filter => {
                    if value.is_truthy() {
                        self.messages
                            .push(self.plan.conversation.messages[index].clone());
                    }
                    self.advance()
                }
                ConversationCallbackOperation::Map => {
                    self.values.push(value);
                    self.advance()
                }
                ConversationCallbackOperation::MapRole(_) => {
                    let transformed = value
                        .as_message_rc()
                        .ok_or_else(|| SemaError::type_error("message", value.type_name()))?;
                    self.messages.push((*transformed).clone());
                    self.advance()
                }
                ConversationCallbackOperation::Find => {
                    if value.is_truthy() {
                        Ok(NativeOutcome::Return(argument))
                    } else {
                        self.advance()
                    }
                }
            },
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{} callback was cancelled ({reason:?})",
                self.plan.operation.name()
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{} callback received an unexpected runtime response",
                self.plan.operation.name()
            ))),
        }
    }
}

impl ConversationCallbackOperation {
    fn name(&self) -> &'static str {
        match self {
            Self::Filter => ConversationCallbackKind::Filter.name(),
            Self::Map => ConversationCallbackKind::Map.name(),
            Self::MapRole(_) => ConversationCallbackKind::MapRole.name(),
            Self::Find => ConversationCallbackKind::Find.name(),
        }
    }
}

/// Parse a message role keyword (`:system`/`:user`/`:assistant`/`:tool`) for the
/// conversation-surgery builtins, erroring with `who` in the message on anything else.
pub(super) fn parse_role(v: &Value, who: &str) -> Result<Role, SemaError> {
    let kw = v
        .as_keyword()
        .ok_or_else(|| SemaError::type_error("keyword", v.type_name()))?;
    match kw.as_str() {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        "tool" => Ok(Role::Tool),
        other => Err(SemaError::eval(format!("{who}: unknown role '{other}'"))),
    }
}

/// Build a `Message` from the tail of a surgery call: either a single message value
/// (`(op conv i msg)`) or a `role`/`content` pair (`(op conv i :system "…")`).
pub(super) fn message_from_tail(tail: &[Value], who: &str) -> Result<Message, SemaError> {
    match tail {
        [m] => m
            .as_message_rc()
            .map(|rc| (*rc).clone())
            .ok_or_else(|| SemaError::type_error("message", m.type_name())),
        [role, content] => Ok(Message {
            role: parse_role(role, who)?,
            content: content
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| content.to_string()),
            images: Vec::new(),
        }),
        _ => Err(SemaError::arity(who, "3-4", tail.len() + 2)),
    }
}

/// Identity key for prompt-algebra dedup/compare: two messages are "the same" when
/// role and content match (images are ignored).
pub(super) fn msg_key(m: &Message) -> (Role, &str) {
    (m.role.clone(), m.content.as_str())
}

/// Fold a completed turn's real `usage` into a conversation's metadata so that
/// `conversation/cost`/`conversation/stats` report actual billed figures. Cost is only
/// accumulated when the model's price is known; if no turn ever contributes a priced
/// usage, `usage-cost` stays absent and `conversation/cost` returns nil.
pub(super) fn accumulate_usage(meta: &mut BTreeMap<String, String>, usage: &Usage) {
    let add_u32 = |meta: &mut BTreeMap<String, String>, key: &str, delta: u32| {
        let prev: u64 = meta.get(key).and_then(|s| s.parse().ok()).unwrap_or(0);
        meta.insert(key.to_string(), (prev + delta as u64).to_string());
    };
    add_u32(meta, "usage-prompt-tokens", usage.prompt_tokens);
    add_u32(meta, "usage-completion-tokens", usage.completion_tokens);
    if let Some(cost) = pricing::calculate_cost(usage) {
        let prev: f64 = meta
            .get("usage-cost")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        meta.insert("usage-cost".to_string(), (prev + cost).to_string());
    }
}

pub(super) fn register(env: &Env) {
    // (conversation/new {:model "..."})
    register_fn(env, "conversation/new", |args| {
        let mut model = String::new();
        let mut metadata = BTreeMap::new();
        if let Some(opts_val) = args.first() {
            if let Some(opts) = opts_val.as_map_rc() {
                model = opts.opt_str("model").unwrap_or_default();
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
    register_runtime_fn_ctx(env, "conversation/say", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
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
                temperature = opts.opt_f64("temperature");
                max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
                system = opts.opt_str("system");
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

        // Build the new conversation (user message + assistant reply appended).
        // Shared by the sync and async paths — conversation state mutation (the
        // history append) happens here, AFTER the response lands (on the VM
        // thread in the async case), never inside the offload.
        let finalize = move |response: ChatResponse| -> Result<Value, SemaError> {
            Ok(conversation_with_exchange(&conv, user_msg, response))
        };

        // Runtime roots and spawned tasks suspend on an External wait; only a
        // host call uses the synchronous adapter.
        #[cfg(not(target_arch = "wasm32"))]
        {
            dispatch_complete_offload(request, CompleteFinalize::new(finalize))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let response = do_complete(request)?;
            track_usage(&response.usage)?;
            finalize(response).map(NativeOutcome::Return)
        }
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
        let text = args.str_at(1, "message/with-image")?.to_string();
        let bv = args.bytes_at(2, "message/with-image")?;

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
        let role_kw = args.keyword_at(1, "conversation/add-message")?;
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
    register_conversation_callback_fn(env, ConversationCallbackKind::Filter);

    // (conversation/map conv f) — transform each message with (f msg), returns list of results
    register_conversation_callback_fn(env, ConversationCallbackKind::Map);

    // (conversation/say-as conv system-prompt "message" opts?) — say with a different system prompt for one turn
    register_runtime_fn_ctx(env, "conversation/say-as", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
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
                temperature = opts.opt_f64("temperature");
                max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
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

        // Build new conversation preserving the original system message (not the
        // override). Shared by the sync and async paths — conversation state
        // mutation (the history append) happens here, AFTER the response lands
        // (on the VM thread in the async case), never inside the offload.
        let finalize = move |response: ChatResponse| -> Result<Value, SemaError> {
            Ok(conversation_with_exchange(&conv, user_msg, response))
        };

        // Runtime roots and spawned tasks suspend on an External wait; only a
        // host call uses the synchronous adapter.
        #[cfg(not(target_arch = "wasm32"))]
        {
            dispatch_complete_offload(request, CompleteFinalize::new(finalize))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let response = do_complete(request)?;
            track_usage(&response.usage)?;
            finalize(response).map(NativeOutcome::Return)
        }
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

    // (conversation/cost conv) — cumulative cost in USD, summed from each turn's actual
    // usage as it was sent (see accumulate_usage in conversation/say). Returns nil when no
    // priced turn has been recorded.
    register_fn(env, "conversation/cost", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/cost", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        match conv
            .metadata
            .get("usage-cost")
            .and_then(|s| s.parse::<f64>().ok())
        {
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
        let vars = args.map_at(1, "prompt/fill")?;
        let messages: Vec<Message> = p
            .messages
            .iter()
            .map(|m| {
                let filled = sema_core::text_util::render_template(&m.content, &vars);
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

    // ---- Conversation inspection (issue #12, Part 3) ----

    // (conversation/length conv) — number of messages
    register_fn(env, "conversation/length", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/length", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        Ok(Value::int(conv.messages.len() as i64))
    });

    // (conversation/turns conv) — number of assistant replies (user/assistant exchanges)
    register_fn(env, "conversation/turns", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/turns", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let turns = conv
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count();
        Ok(Value::int(turns as i64))
    });

    // (conversation/models-used conv) — list of models (the conversation carries one)
    register_fn(env, "conversation/models-used", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "conversation/models-used",
                "1",
                args.len(),
            ));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        if conv.model.is_empty() {
            Ok(Value::list(Vec::new()))
        } else {
            Ok(Value::list(vec![Value::string(&conv.model)]))
        }
    });

    // (conversation/stats conv) — aggregate report. Token/cost figures come from the
    // real usage accumulated by conversation/say (see the usage-* metadata written there);
    // they are 0 / nil when no priced turn has been sent.
    register_fn(env, "conversation/stats", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/stats", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let turns = conv
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count() as i64;
        let prompt_tokens: i64 = conv
            .metadata
            .get("usage-prompt-tokens")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let completion_tokens: i64 = conv
            .metadata
            .get("usage-completion-tokens")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let cost = conv
            .metadata
            .get("usage-cost")
            .and_then(|s| s.parse::<f64>().ok());

        let mut tokens = BTreeMap::new();
        tokens.insert(Value::keyword("prompt"), Value::int(prompt_tokens));
        tokens.insert(Value::keyword("completion"), Value::int(completion_tokens));
        tokens.insert(
            Value::keyword("total"),
            Value::int(prompt_tokens + completion_tokens),
        );

        let models = if conv.model.is_empty() {
            Value::list(Vec::new())
        } else {
            Value::list(vec![Value::string(&conv.model)])
        };

        let mut stats = BTreeMap::new();
        stats.insert(
            Value::keyword("messages"),
            Value::int(conv.messages.len() as i64),
        );
        stats.insert(Value::keyword("turns"), Value::int(turns));
        stats.insert(Value::keyword("tokens"), Value::map(tokens));
        stats.insert(
            Value::keyword("cost"),
            cost.map(Value::float).unwrap_or_else(Value::nil),
        );
        stats.insert(Value::keyword("models"), models);
        Ok(Value::map(stats))
    });

    // ---- Conversation surgery (issue #12, Part 3) ----

    // (conversation/remove conv idx) — drop the message at idx
    register_fn(env, "conversation/remove", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/remove", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let idx = args.int_at(1, "conversation/remove")?;
        let mut messages = conv.messages.clone();
        if idx < 0 || idx as usize >= messages.len() {
            return Err(SemaError::eval(format!(
                "conversation/remove: index {idx} out of bounds (length {})",
                messages.len()
            )));
        }
        messages.remove(idx as usize);
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/insert conv idx msg) | (conversation/insert conv idx :role "content")
    register_fn(env, "conversation/insert", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("conversation/insert", "3-4", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let idx = args.int_at(1, "conversation/insert")?;
        let msg = message_from_tail(&args[2..], "conversation/insert")?;
        let mut messages = conv.messages.clone();
        // idx == len is allowed (append); anything past that is out of bounds.
        if idx < 0 || idx as usize > messages.len() {
            return Err(SemaError::eval(format!(
                "conversation/insert: index {idx} out of bounds (length {})",
                messages.len()
            )));
        }
        messages.insert(idx as usize, msg);
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/replace conv idx msg) | (conversation/replace conv idx :role "content")
    register_fn(env, "conversation/replace", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("conversation/replace", "3-4", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let idx = args.int_at(1, "conversation/replace")?;
        let msg = message_from_tail(&args[2..], "conversation/replace")?;
        let mut messages = conv.messages.clone();
        if idx < 0 || idx as usize >= messages.len() {
            return Err(SemaError::eval(format!(
                "conversation/replace: index {idx} out of bounds (length {})",
                messages.len()
            )));
        }
        messages[idx as usize] = msg;
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/map-role conv :role f) — transform only messages of `role` with (f msg),
    // which must return a message; other messages pass through unchanged.
    register_conversation_callback_fn(env, ConversationCallbackKind::MapRole);

    // ---- Conversation search (issue #12, Part 3) ----

    // (conversation/search conv query) — case-insensitive substring search over message
    // content; returns a list of {:index :role :content} maps for each hit.
    register_fn(env, "conversation/search", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/search", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let query = args.str_at(1, "conversation/search")?.to_lowercase();
        let mut hits = Vec::new();
        for (i, m) in conv.messages.iter().enumerate() {
            if m.content.to_lowercase().contains(&query) {
                let mut hit = BTreeMap::new();
                hit.insert(Value::keyword("index"), Value::int(i as i64));
                hit.insert(Value::keyword("role"), Value::keyword(&m.role.to_string()));
                hit.insert(Value::keyword("content"), Value::string(&m.content));
                hits.push(Value::map(hit));
            }
        }
        Ok(Value::list(hits))
    });

    // (conversation/find conv pred) — first message where (pred msg) is truthy, else nil
    register_conversation_callback_fn(env, ConversationCallbackKind::Find);

    // ---- Prompt algebra (issue #12, Part 7) — exact (role, content) matching ----

    // (prompt/diff a b) — {:added [msgs only in b] :removed [msgs only in a]}
    register_fn(env, "prompt/diff", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/diff", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let a_keys: Vec<_> = a.messages.iter().map(msg_key).collect();
        let b_keys: Vec<_> = b.messages.iter().map(msg_key).collect();
        let added: Vec<Value> = b
            .messages
            .iter()
            .filter(|m| !a_keys.contains(&msg_key(m)))
            .map(|m| Value::message(m.clone()))
            .collect();
        let removed: Vec<Value> = a
            .messages
            .iter()
            .filter(|m| !b_keys.contains(&msg_key(m)))
            .map(|m| Value::message(m.clone()))
            .collect();
        let mut out = BTreeMap::new();
        out.insert(Value::keyword("added"), Value::list(added));
        out.insert(Value::keyword("removed"), Value::list(removed));
        Ok(Value::map(out))
    });

    // (prompt/union a b) — messages of a then b, de-duplicated, order preserved
    register_fn(env, "prompt/union", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/union", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let mut seen: Vec<(Role, String)> = Vec::new();
        let mut messages = Vec::new();
        for m in a.messages.iter().chain(b.messages.iter()) {
            let key = (m.role.clone(), m.content.clone());
            if !seen.contains(&key) {
                seen.push(key);
                messages.push(m.clone());
            }
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/intersection a b) — messages present in both (order/dedup from a)
    register_fn(env, "prompt/intersection", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/intersection", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let b_keys: Vec<_> = b.messages.iter().map(msg_key).collect();
        let mut seen: Vec<(Role, String)> = Vec::new();
        let mut messages = Vec::new();
        for m in &a.messages {
            let key = (m.role.clone(), m.content.clone());
            if b_keys.contains(&msg_key(m)) && !seen.contains(&key) {
                seen.push(key);
                messages.push(m.clone());
            }
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/difference a b) — messages in a but not b (order/dedup from a)
    register_fn(env, "prompt/difference", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/difference", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let b_keys: Vec<_> = b.messages.iter().map(msg_key).collect();
        let mut seen: Vec<(Role, String)> = Vec::new();
        let mut messages = Vec::new();
        for m in &a.messages {
            let key = (m.role.clone(), m.content.clone());
            if !b_keys.contains(&msg_key(m)) && !seen.contains(&key) {
                seen.push(key);
                messages.push(m.clone());
            }
        }
        Ok(Value::prompt(Prompt { messages }))
    });
}
