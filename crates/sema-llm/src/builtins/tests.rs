use super::*;
use sema_core::{intern, Lambda};
use serde_json::json;

#[test]
fn accumulate_into_counts_cache_hits_without_calls() {
    let slot = Rc::new(RefCell::new(LeafUsage::default()));
    accumulate_into(&slot, &Usage::default(), None);
    let usage = slot.borrow().clone();
    assert_eq!(usage.cache_hits, 1);
    assert_eq!(usage.calls, 0);
    assert_eq!(usage.cost_usd, None);
}

#[test]
fn merge_leaf_propagates_cache_hits() {
    let mut dst = LeafUsage::default();
    let src = LeafUsage {
        cache_hits: 2,
        ..LeafUsage::default()
    };
    merge_leaf(&mut dst, &src);
    assert_eq!(dst.cache_hits, 2);
    assert_eq!(dst.calls, 0);
}

fn runtime_quantum_probe(ctx: &EvalContext, _args: &[Value]) -> sema_core::runtime::NativeResult {
    Ok(sema_core::runtime::NativeOutcome::Return(Value::bool(
        ctx.runtime_quantum_active(),
    )))
}

fn invoke_context_probe(env: &Env, name: &str, eval_context: &EvalContext) -> Value {
    use sema_core::runtime::{
        CancellationView, NativeCallContext, NativeOutcome, TaskContextHandle,
    };

    let callable = env.get(intern(name)).expect("registered context probe");
    let native = callable.as_native_fn_rc().expect("probe is a native");
    let task_context = TaskContextHandle::default();
    let mut call_context = NativeCallContext {
        hof_host: None,
        eval_context,
        task_context,
        call_env: None,
        cancellation: CancellationView::default(),
    };
    match native
        .invoke_runtime(&mut call_context, &[])
        .expect("probe invocation succeeds")
    {
        NativeOutcome::Return(value) => value,
        _ => panic!("context probe must return directly"),
    }
}

#[test]
fn runtime_context_helpers_use_the_invocation_context() {
    let eval_context = EvalContext::new();
    let env = Env::new();
    register_runtime_fn_ctx(&env, "runtime-context-probe", runtime_quantum_probe);
    register_runtime_fn_ctx_gated_as(
        &env,
        &sema_core::Sandbox::allow_all(),
        sema_core::Caps::NETWORK,
        "gated-runtime-context-probe",
        "gated-runtime-context-probe",
        runtime_quantum_probe,
    );

    let _quantum = eval_context
        .enter_runtime_quantum()
        .expect("probe owns the runtime quantum");
    assert_eq!(
        invoke_context_probe(&env, "runtime-context-probe", &eval_context),
        Value::bool(true)
    );
    assert_eq!(
        invoke_context_probe(&env, "gated-runtime-context-probe", &eval_context),
        Value::bool(true)
    );
}

#[test]
fn conversation_callback_driver_traces_retained_values() {
    use sema_core::runtime::Trace;

    let active_argument = Value::int(40);
    let callback = Value::int(41);
    let accumulated = Value::int(42);
    let driver = ConversationCallbackDriver {
        plan: ConversationCallbackPlan {
            conversation: Rc::new(Conversation {
                messages: Vec::new(),
                model: String::new(),
                metadata: BTreeMap::new(),
            }),
            callback: callback.clone(),
            operation: ConversationCallbackOperation::Map,
        },
        next_message: 0,
        active_message: Some(0),
        active_argument: Some(active_argument.clone()),
        values: vec![accumulated.clone()],
        messages: Vec::new(),
    };
    let mut traced = Vec::new();
    assert!(driver.trace(&mut |edge| {
        if let sema_core::cycle::GcEdge::Value(value) = edge {
            traced.push(value.clone());
        }
    }));
    assert_eq!(traced, vec![callback, active_argument, accumulated]);
}

/// The `llm/with-*` teardown continuation captures only non-`Value` scope
/// state (a `FnOnce` over bools/ints/`Rc<BudgetFrame>`/provider names), so it
/// exposes ZERO GC edges — the runtime traces it without visiting any `Value`.
#[test]
fn scope_guard_continuation_holds_no_gc_edges() {
    use sema_core::runtime::Trace;
    let fired = std::rc::Rc::new(std::cell::Cell::new(false));
    let fired2 = fired.clone();
    let cont = ScopeGuardContinuation {
        teardown: Some(Box::new(move || fired2.set(true))),
    };
    let mut edges = 0usize;
    assert!(cont.trace(&mut |_| edges += 1));
    assert_eq!(edges, 0, "teardown continuation must expose no Value edges");
    // Sanity: the captured teardown is a real closure that runs once.
    (cont.teardown.unwrap())();
    assert!(fired.get());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn runtime_extract_finalizer_traces_pending_validation_values() {
    use sema_core::runtime::Trace;

    let driver = RuntimeExtractDriver {
        cfg: ExtractConfig {
            schema: Value::int(1),
            schema_desc: String::new(),
            system: String::new(),
            model: String::new(),
            messages: Vec::new(),
            validate: true,
            max_retries: 1,
            reask: true,
        },
        attempt: 0,
        last_validation_error: String::new(),
        last_response_content: String::new(),
        result: Some(Value::int(2)),
        steps: VecDeque::from([ExtractionValidationStep::Predicate {
            callable: Value::int(3),
            argument: Value::int(4),
            key_name: "field".to_string(),
            failure_message: "invalid".to_string(),
        }]),
        errors: Vec::new(),
        phase: RuntimeExtractPhase::Ready,
        llm_scope: LlmDynScope::default(),
        otel_scope: sema_otel::OtelTaskCtx::default(),
        usage_accum_slot: None,
    };
    let finalize = CompleteFinalize::runtime(Box::new(driver));
    let mut edges = 0;
    assert!(finalize.trace(&mut |_| edges += 1));
    assert_eq!(edges, 4, "schema, result, predicate, and argument are live");
}

/// The cooperative tool-round continuation exposes exactly its live `Value`
/// edges: each tool, the `:on-tool-call` callback, and — while a call is in
/// flight — its args value plus the pending `(handler, args)`. `ToolCall` and the
/// (detached) `ToolSpan` hold no `Value`, so they contribute no edges.
#[test]
fn exec_tools_continuation_traces_its_value_edges() {
    use sema_core::runtime::Trace;
    fn edges(c: &ExecToolsContinuation) -> usize {
        let mut n = 0usize;
        assert!(c.trace(&mut |_| n += 1));
        n
    }
    let tc = ToolCall {
        id: "call_1".into(),
        name: "t".into(),
        arguments: json!({"a": 1}),
        thought_signature: None,
    };
    // Idle (no active call): 2 tools + 1 callback = 3 edges.
    let mut cont = ExecToolsContinuation {
        token: 1,
        tools: vec![Value::int(1), Value::int(2)],
        on_tool_call: Some(Value::int(3)),
        remaining: std::collections::VecDeque::new(),
        denied: BTreeMap::new(),
        active: None,
        phase: ToolPhase::Handler,
    };
    assert_eq!(edges(&cont), 3);
    // With a call in flight: + args_value + handler + 2 handler args = 4 more.
    cont.active = Some(ActiveCall {
        tc,
        pending_handler: Some((Value::int(4), vec![Value::int(5), Value::int(6)])),
        pending_error: None,
        validation_steps: VecDeque::new(),
        validation_errors: Vec::new(),
        args_value: Value::int(7),
        args_json: "{}".into(),
        span: None,
        started: None,
    });
    assert_eq!(edges(&cont), 7);
    cont.active
        .as_mut()
        .expect("active call")
        .validation_steps
        .push_back(ExtractionValidationStep::Predicate {
            callable: Value::int(8),
            argument: Value::int(9),
            key_name: "field".into(),
            failure_message: "invalid".into(),
        });
    assert_eq!(edges(&cont), 9);
    // No callback → one fewer edge.
    cont.on_tool_call = None;
    assert_eq!(edges(&cont), 8);
}

fn usage(prompt: u32, completion: u32) -> Usage {
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        model: "fake-model".into(),
        ..Usage::default()
    }
}

#[test]
fn accumulate_into_sums_tokens_cost_and_calls() {
    let slot = Rc::new(RefCell::new(LeafUsage::default()));
    accumulate_into(&slot, &usage(10, 5), Some(0.001));
    accumulate_into(&slot, &usage(100, 50), Some(0.002));
    let u = slot.borrow();
    assert_eq!(u.input_tokens, 110, "tokens sum across calls");
    assert_eq!(u.output_tokens, 55);
    assert!((u.cost_usd.unwrap() - 0.003).abs() < 1e-9, "cost sums");
    assert_eq!(u.calls, 2);
    assert_eq!(u.model, "fake-model");
}

#[test]
fn accumulate_into_skips_cache_hit_zero_usage() {
    // A cache hit is all-zero + unpriced — it must NOT count as a call (no phantom
    // zero Budget event downstream), per the cache-hit-zero-usage invariant.
    let slot = Rc::new(RefCell::new(LeafUsage::default()));
    accumulate_into(&slot, &Usage::default(), None);
    let u = slot.borrow();
    assert_eq!(u.calls, 0, "cache hit doesn't bump calls");
    assert_eq!(u.input_tokens, 0);
    assert!(u.cost_usd.is_none());
}

#[test]
fn accumulate_into_unpriced_call_counts_but_leaves_cost_none() {
    // An unpriced (no pricing-table entry) but token-bearing call IS a call; cost stays
    // genuinely absent rather than $0, then a later priced call seeds the running sum.
    let slot = Rc::new(RefCell::new(LeafUsage::default()));
    accumulate_into(&slot, &usage(7, 3), None);
    assert_eq!(slot.borrow().calls, 1);
    assert!(
        slot.borrow().cost_usd.is_none(),
        "unpriced ⇒ cost still None"
    );
    accumulate_into(&slot, &usage(1, 1), Some(0.005));
    assert!((slot.borrow().cost_usd.unwrap() - 0.005).abs() < 1e-9);
    assert_eq!(slot.borrow().calls, 2);
}

#[test]
fn usage_scope_nests_and_restores_the_active_frame() {
    let outer = open_usage_scope();
    let outer_slot = current_usage_accum().expect("outer scope active");
    {
        let _inner = open_usage_scope();
        let inner_slot = current_usage_accum().expect("inner scope active");
        assert!(
            !Rc::ptr_eq(&inner_slot, &outer_slot),
            "a nested scope installs a distinct frame"
        );
    }
    // inner dropped → the outer frame is the active one again
    assert!(
        Rc::ptr_eq(&current_usage_accum().expect("outer restored"), &outer_slot),
        "dropping the inner scope restores the outer frame"
    );
    drop(outer);
}

#[test]
fn open_usage_scope_collects_completions_made_while_alive() {
    let scope = open_usage_scope();
    // Simulate two completions folding into the active frame (as track_usage does).
    let slot = current_usage_accum().expect("scope active");
    accumulate_into(&slot, &usage(20, 10), Some(0.01));
    accumulate_into(&slot, &usage(4, 2), None);
    let u = scope.usage();
    assert_eq!(u.input_tokens, 24);
    assert_eq!(u.output_tokens, 12);
    assert_eq!(u.calls, 2);
    drop(scope);
    assert!(
        current_usage_accum().is_none(),
        "dropping the only scope leaves no active frame"
    );
}

#[test]
fn llm_dynamic_scope_capture_copies_configuration_but_not_last_usage() {
    let fallback = vec![FallbackEntry {
        provider: "scoped-provider".to_string(),
        model: Some("scoped-model".to_string()),
    }];
    let last_usage = usage(17, 9);
    FALLBACK_CHAIN.with(|chain| *chain.borrow_mut() = Some(fallback));
    RATE_LIMIT_RPS.with(|rate| rate.set(Some(7.0)));
    set_rate_limit_last_value(41);
    RETRY_BASE_MS.with(|base| base.set(23));
    NETWORK_MAX_RETRIES.with(|retries| retries.set(5));
    LAST_USAGE.with(|usage| *usage.borrow_mut() = Some(last_usage.clone()));
    let captured = capture_llm_scope();

    FALLBACK_CHAIN.with(|chain| *chain.borrow_mut() = None);
    RATE_LIMIT_RPS.with(|rate| rate.set(None));
    RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = None);
    RETRY_BASE_MS.with(|base| base.set(500));
    NETWORK_MAX_RETRIES.with(|retries| retries.set(3));
    LAST_USAGE.with(|usage| *usage.borrow_mut() = None);

    let displaced = install_llm_scope(captured);
    assert_eq!(
        FALLBACK_CHAIN.with(|chain| chain.borrow().as_ref().unwrap()[0].provider.clone()),
        "scoped-provider"
    );
    assert_eq!(RATE_LIMIT_RPS.with(Cell::get), Some(7.0));
    assert_eq!(rate_limit_last_value(), 41);
    assert_eq!(RETRY_BASE_MS.with(Cell::get), 23);
    assert_eq!(NETWORK_MAX_RETRIES.with(Cell::get), 5);
    assert!(LAST_USAGE.with(|usage| usage.borrow().is_none()));

    let _ = install_llm_scope(displaced);
}

#[test]
fn llm_dynamic_scope_take_install_preserves_own_last_usage() {
    let last_usage = usage(17, 9);
    LAST_USAGE.with(|usage| *usage.borrow_mut() = Some(last_usage.clone()));

    let taken = take_llm_scope();
    assert!(LAST_USAGE.with(|usage| usage.borrow().is_none()));

    let displaced = install_llm_scope(taken);
    LAST_USAGE.with(|usage| {
        let usage = usage.borrow();
        let restored = usage.as_ref().expect("task usage restored");
        assert_eq!(restored.prompt_tokens, last_usage.prompt_tokens);
        assert_eq!(restored.completion_tokens, last_usage.completion_tokens);
        assert_eq!(restored.model, last_usage.model);
    });

    let _ = install_llm_scope(displaced);
}

#[test]
fn captured_rate_limit_siblings_share_one_reservation_cursor() {
    RATE_LIMIT_RPS.with(|rate| rate.set(Some(10.0)));
    set_rate_limit_last_value(0);
    let first_child = capture_llm_scope();
    let second_child = capture_llm_scope();
    let parent = take_llm_scope();
    RATE_LIMIT_RPS.with(|rate| rate.set(None));
    RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = None);

    let displaced = install_llm_scope(first_child);
    assert_eq!(reserve_rate_limit_wait_ms(), 0);
    let _first_child = take_llm_scope();
    let _ = install_llm_scope(displaced);

    let displaced = install_llm_scope(second_child);
    let second_wait = reserve_rate_limit_wait_ms();
    assert!(
        (90..=110).contains(&second_wait),
        "sibling must reserve after the first shared slot, got {second_wait}ms"
    );
    let _second_child = take_llm_scope();
    let _ = install_llm_scope(displaced);

    let _ = install_llm_scope(parent);
    RATE_LIMIT_RPS.with(|rate| rate.set(None));
    RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = None);
}

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
    set_rate_limit_last_value(now + 1_000_000);
    let start = std::time::Instant::now();
    enforce_rate_limit();
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "backward clock should not cause a long sleep"
    );
    RATE_LIMIT_RPS.with(|r| r.set(None));
    RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = None);
}

#[test]
fn reserve_rate_limit_wait_ms_stays_zero_with_no_gate() {
    RATE_LIMIT_RPS.with(|r| r.set(None));
    RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = None);
    assert_eq!(reserve_rate_limit_wait_ms(), 0);
}

#[test]
fn reserve_rate_limit_wait_ms_stages_a_concurrent_burst() {
    // Three back-to-back reservations at rps=10 (100ms interval) with no
    // real time elapsing between them: the first is free, and each next
    // one is pushed exactly one interval further out than the last —
    // proving concurrent async dispatches get staggered instead of all
    // computing the same wait against a stale `RATE_LIMIT_LAST`.
    RATE_LIMIT_RPS.with(|r| r.set(Some(10.0)));
    set_rate_limit_last_value(0);
    let first = reserve_rate_limit_wait_ms();
    let second = reserve_rate_limit_wait_ms();
    let third = reserve_rate_limit_wait_ms();
    assert_eq!(first, 0, "no prior reservation: the gate is clear");
    assert!(
        (90..=110).contains(&second),
        "second reservation should land ~100ms out, got {second}"
    );
    assert!(
        (190..=210).contains(&third),
        "third reservation should land ~200ms out, got {third}"
    );
    RATE_LIMIT_RPS.with(|r| r.set(None));
    RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = None);
}

#[test]
fn reserve_rate_limit_wait_ms_survives_backward_clock() {
    // Mirrors `enforce_rate_limit_survives_backward_clock`: a reserved slot
    // far in the future (wall clock jumped backward since it was stamped)
    // must not wedge this call behind a real multi-minute wait.
    RATE_LIMIT_RPS.with(|r| r.set(Some(10.0)));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    set_rate_limit_last_value(now + 1_000_000);
    let wait = reserve_rate_limit_wait_ms();
    assert!(
        wait < 1_000,
        "backward clock should not produce a huge reserved wait, got {wait}ms"
    );
    RATE_LIMIT_RPS.with(|r| r.set(None));
    RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = None);
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

#[test]
fn complete_finalize_traces_values_retained_by_its_closure() {
    let retained = make_lambda(&["value"]);
    let finalize =
        CompleteFinalize::with_values(|_response| Ok(Value::nil()), vec![retained.clone()]);
    let mut saw_retained = false;

    sema_core::runtime::Trace::trace(&finalize, &mut |edge| {
        if let sema_core::cycle::GcEdge::Value(value) = edge {
            saw_retained |= value == &retained;
        }
    });

    assert!(saw_retained, "the retained Value must be visible to CORE-2");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn runtime_complete_driver_traces_its_finalizer_values() {
    let retained = make_lambda(&["value"]);
    let driver = RuntimeCompleteDriver {
        plan: CompleteOffloadPlan {
            chain: Vec::new(),
            explicit_fallback: false,
            request: ChatRequest::new(String::new(), Vec::new()),
            max_retries: 0,
            retry_base_ms: 0,
            rate_limit_wait_ms: 0,
            span: sema_otel::llm_span_detached("chat"),
            cache_key: None,
            cassette_record_key: None,
            cassette_scope: None,
            request_for_messages: ChatRequest::new(String::new(), Vec::new()),
        },
        finalize: CompleteFinalize::with_values(
            |_response| Ok(Value::nil()),
            vec![retained.clone()],
        ),
        next_provider: 0,
        last_error: None,
        phase: RuntimeCompletePhase::Ready,
        cache_peeked: false,
        usage_accum_slot: None,
        budget_slot: None,
    };
    let mut saw_retained = false;

    sema_core::runtime::Trace::trace(&driver, &mut |edge| {
        if let sema_core::cycle::GcEdge::Value(value) = edge {
            saw_retained |= value == &retained;
        }
    });

    assert!(
        saw_retained,
        "the runtime driver must retain the finalizer's CORE-2 edge"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn complete_attempt_decoder_delivers_one_owned_result() {
    use sema_core::runtime::{
        CancellationView, CompletionDecoder, NativeCallContext, SendPayload, TaskContextHandle,
    };

    let slot = Rc::new(RefCell::new(None));
    let decoder = Box::new(CompleteAttemptDecoder {
        slot: Rc::clone(&slot),
    });
    let outcome = CompleteOutcome {
        resp: ChatResponse {
            content: "done".to_string(),
            role: "assistant".to_string(),
            model: "fake-model".to_string(),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            stop_reason: Some("end_turn".to_string()),
        },
        serving_provider: "fake".to_string(),
        serving_model: "fake-model".to_string(),
        retry_events: Vec::new(),
    };
    let eval_context = EvalContext::new();
    let task_context = TaskContextHandle::default();
    let mut call_context = NativeCallContext {
        hof_host: None,
        eval_context: &eval_context,
        task_context,
        call_env: None,
        cancellation: CancellationView::default(),
    };

    let decoded = CompletionDecoder::decode(
        decoder,
        &mut call_context,
        Ok(Box::new(Ok::<CompleteOutcome, LlmError>(outcome)) as SendPayload),
    )
    .expect("matching completion payload decodes");

    assert_eq!(decoded, Value::nil());
    assert!(matches!(slot.borrow_mut().take(), Some(Ok(_))));
    assert!(slot.borrow_mut().take().is_none(), "slot is consumed once");
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

// -- tool-call argument ordering (json_args_to_sema) --
// These pin that JSON arguments bind to handler params by *declaration order*,
// not alphabetically. The binding lives in `json_args_to_sema`; the handler is
// applied later via the canonical evaluator callback (covered end-to-end by the
// FakeProvider agent tests in `crates/sema/tests/llm_fake_test.rs`).

#[test]
fn test_tool_args_bind_in_declaration_order() {
    // Params (path, content): alphabetical order would swap them.
    let handler = Value::lambda(Lambda {
        params: vec![intern("path"), intern("content")],
        rest_param: None,
        body: vec![Value::symbol("path")],
        env: Env::new(),
        name: Some(intern("write-file-handler")),
    });
    let params = make_param_map(&["path", "content"]);
    let args = json!({"path": "/tmp/test.txt", "content": "file body here"});

    let result = json_args_to_sema(&params, &args, &handler);

    // Declaration order (path, content), not alphabetical (content, path).
    assert_eq!(result[0], Value::string("/tmp/test.txt"));
    assert_eq!(result[1], Value::string("file body here"));
}

#[test]
fn test_tool_args_reverse_alpha_order() {
    // Params (z_last, a_first): exact reverse of alphabetical.
    let handler = Value::lambda(Lambda {
        params: vec![intern("z_last"), intern("a_first")],
        rest_param: None,
        body: vec![Value::symbol("z_last")],
        env: Env::new(),
        name: Some(intern("test-handler")),
    });
    let params = make_param_map(&["z_last", "a_first"]);
    let args = json!({"z_last": "ZLAST", "a_first": "AFIRST"});

    let result = json_args_to_sema(&params, &args, &handler);

    // z_last is declared first, so it must be arg 0 — not alphabetical.
    assert_eq!(result[0], Value::string("ZLAST"));
    assert_eq!(result[1], Value::string("AFIRST"));
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
fn test_validate_extraction_bare_keyword_spec_type_checked() {
    // {:total :number} is shorthand for {:type :number} — a string value
    // like "$10.00" must FAIL, not pass with key-presence-only checking.
    let schema = {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("total"), Value::keyword("number"));
        Value::map(map)
    };
    let bad = {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("total"), Value::string("$10.00"));
        Value::map(map)
    };
    let err = validate_extraction(&bad, &schema).unwrap_err();
    assert!(err.contains("expected number"), "got: {err}");

    let good = {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("total"), Value::float(10.0));
        Value::map(map)
    };
    assert!(validate_extraction(&good, &schema).is_ok());
}

#[test]
fn test_format_schema_bare_keyword_spec_renders_type() {
    let schema = {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("total"), Value::keyword("number"));
        Value::map(map)
    };
    let rendered = format_schema(&schema);
    assert!(rendered.contains("\"total\": <number>"), "got: {rendered}");
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

/// A cached tool-call turn must replay AS a tool-call turn.
///
/// The cache entry had no tool-call field and the hit path hard-coded an
/// empty vector, so an agent round inside `llm/with-cache` was stored as
/// its (empty) content and replayed as a final answer: `run_tool_loop` saw
/// no tool calls, stopped, and returned "" without running anything or
/// raising. Entries persist to disk, so one run poisoned every later run
/// within the TTL.
#[test]
fn cached_tool_call_turns_survive_the_round_trip() {
    let key = "test-tool-call-round-trip";
    let response = ChatResponse {
        content: String::new(),
        role: "assistant".to_string(),
        model: "m".to_string(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: serde_json::json!({"city": "Oslo"}),
            thought_signature: None,
        }],
        usage: Usage::default(),
        stop_reason: Some("tool_use".to_string()),
    };
    store_cached(key, &response, "fake");

    let cached = CACHE_MEM
        .with(|c| c.borrow().get(key).cloned())
        .expect("entry was just stored");
    let replayed = cache_hit_response(cached, "m".to_string());

    assert_eq!(replayed.tool_calls.len(), 1, "the tool call must survive");
    assert_eq!(replayed.tool_calls[0].name, "get_weather");
    assert_eq!(replayed.tool_calls[0].id, "call_1");
    assert_eq!(replayed.tool_calls[0].arguments["city"], "Oslo");
    // Still a cache hit: no provider call happened, so usage stays zero.
    assert_eq!(replayed.usage.prompt_tokens, 0);
    assert_eq!(replayed.usage.completion_tokens, 0);
}

/// Entries written before the field existed must still load.
#[test]
fn cache_entries_without_tool_calls_still_deserialize() {
    let legacy =
        r#"{"content":"hi","model":"m","prompt_tokens":1,"completion_tokens":2,"cached_at":0}"#;
    let parsed: CachedResponse =
        serde_json::from_str(legacy).expect("a pre-existing entry must still load");
    assert_eq!(parsed.content, "hi");
    assert!(parsed.tool_calls.is_empty());
}
