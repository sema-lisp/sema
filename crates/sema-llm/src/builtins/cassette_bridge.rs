use super::*;

pub(super) fn current_cassette_scope() -> Option<CassetteScope> {
    CASSETTE.with(|c| c.borrow().clone())
}

pub(super) fn install_cassette_scope(scope: Option<CassetteScope>) -> Option<CassetteScope> {
    CASSETTE.with(|c| std::mem::replace(&mut *c.borrow_mut(), scope))
}

pub(super) fn cassette_scope(cassette: crate::cassette::Cassette) -> CassetteScope {
    Rc::new(CassetteState::new(cassette))
}

/// Persist a cassette's pending entries OFF the runtime quantum. Renders the pending
/// NDJSON on the VM thread (bounded), then appends on a blocking-tier worker when a
/// quantum is active (best-effort, matching the tape's existing trust model — the
/// durable record→replay path tears down after a suspended provider call, so it
/// appends synchronously here off the quantum), or synchronously on the host thread
/// otherwise.
pub(super) fn persist_cassette_off_quantum(cassette: &mut crate::cassette::Cassette) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if sema_core::in_runtime_quantum() {
            if let Some((path, encoded)) = cassette.take_pending_append() {
                sema_io::io_spawn_blocking(move || {
                    let _ = crate::cassette::append_ndjson(&path, &encoded);
                });
            }
            return;
        }
    }
    let _ = cassette.save();
}

/// Install an already-loaded cassette into a fresh scope, disabling the response
/// cache for its dynamic extent (a cache hit would short-circuit before the tape —
/// see [`crate::cassette`]), and return the teardown that flushes the tape (off the
/// quantum) and restores the prior cassette + cache state. Shared by both ABIs of
/// `llm/with-cassette`.
pub(super) fn install_loaded_cassette(cassette: crate::cassette::Cassette) -> Box<dyn FnOnce()> {
    let active = cassette_scope(cassette);
    let prev_cassette = install_cassette_scope(Some(Rc::clone(&active)));
    let prev_cache = CACHE_ENABLED.with(|c| c.replace(false));
    Box::new(move || {
        {
            let mut guard = active.borrow_mut();
            persist_cassette_off_quantum(&mut guard);
        }
        install_cassette_scope(prev_cassette);
        CACHE_ENABLED.with(|c| c.set(prev_cache));
    })
}

/// Parse `(llm/with-cassette "path" [{:mode :auto}] thunk)` → (path, mode, body thunk).
pub(super) fn parse_with_cassette_args(
    args: &[Value],
) -> Result<(std::path::PathBuf, crate::cassette::CassetteMode, Value), SemaError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity("llm/with-cassette", "2 or 3", args.len()));
    }
    let path = args.str_at(0, "llm/with-cassette")?;
    let (mode, body_fn) = if args.len() == 3 {
        let opts = args[1]
            .as_map_ref()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let mode = opts
            .opt_name("mode")
            .map(|s| crate::cassette::CassetteMode::parse(&s))
            .unwrap_or(crate::cassette::CassetteMode::Auto);
        (mode, args[2].clone())
    } else {
        (crate::cassette::CassetteMode::Auto, args[1].clone())
    };
    if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
        return Err(SemaError::type_error("function", body_fn.type_name()));
    }
    Ok((std::path::PathBuf::from(path), mode, body_fn))
}

/// Parse `(llm/cassette-load "path" [{:mode :replay}])` → (path, mode).
pub(super) fn parse_cassette_load_args(
    args: &[Value],
) -> Result<(std::path::PathBuf, crate::cassette::CassetteMode), SemaError> {
    if args.is_empty() || args.len() > 2 {
        return Err(SemaError::arity("llm/cassette-load", "1 or 2", args.len()));
    }
    let path = args.str_at(0, "llm/cassette-load")?;
    let mode = if args.len() == 2 {
        let opts = args[1]
            .as_map_ref()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        opts.opt_name("mode")
            .map(|s| crate::cassette::CassetteMode::parse(&s))
            .unwrap_or(crate::cassette::CassetteMode::Auto)
    } else {
        crate::cassette::CassetteMode::Auto
    };
    Ok((std::path::PathBuf::from(path), mode))
}

/// Runtime-ABI arm of `llm/with-cassette`: OFFLOAD the tape load off the quantum, then
/// resume by installing the scope and calling the body under a scope-teardown guard.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn with_cassette_runtime(args: &[Value]) -> sema_core::runtime::NativeResult {
    let (path, mode, body_fn) = parse_with_cassette_args(args)?;
    suspend_cassette_load(path, mode, CassetteLoadThen::WithBody(body_fn))
}

/// wasm has no cooperative-runtime External wait: load synchronously and call the body.
#[cfg(target_arch = "wasm32")]
pub(super) fn with_cassette_runtime(args: &[Value]) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{NativeCall, NativeOutcome};
    let (path, mode, body_fn) = parse_with_cassette_args(args)?;
    let cassette = crate::cassette::Cassette::load(path, mode);
    let teardown = install_loaded_cassette(cassette);
    Ok(NativeOutcome::Call(NativeCall {
        callable: body_fn,
        args: Vec::new(),
        continuation: Box::new(ScopeGuardContinuation {
            teardown: Some(teardown),
        }),
    }))
}

/// Runtime-ABI arm of `llm/cassette-load`: OFFLOAD the tape load off the quantum, then
/// install it in the ambient scope.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn cassette_load_runtime(args: &[Value]) -> sema_core::runtime::NativeResult {
    let (path, mode) = parse_cassette_load_args(args)?;
    suspend_cassette_load(path, mode, CassetteLoadThen::Install)
}

/// wasm has no cooperative-runtime External wait: load synchronously and install.
#[cfg(target_arch = "wasm32")]
pub(super) fn cassette_load_runtime(args: &[Value]) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::NativeOutcome;
    let (path, mode) = parse_cassette_load_args(args)?;
    let cassette = crate::cassette::Cassette::load(path, mode);
    install_cassette(cassette);
    Ok(NativeOutcome::Return(Value::nil()))
}

pub(super) fn cassette_decide(key: &str) -> Option<crate::cassette::Decision> {
    let scope = current_cassette_scope()?;
    let decision = scope.borrow().decide(key);
    Some(decision)
}

pub(super) fn cassette_record(entry: crate::cassette::TapeEntry) {
    if let Some(scope) = current_cassette_scope() {
        scope.borrow_mut().record_entry(entry);
    }
}

pub(super) fn cassette_scope_record(
    scope: &Option<CassetteScope>,
    entry: crate::cassette::TapeEntry,
) {
    if let Some(scope) = scope {
        scope.borrow_mut().record_entry(entry);
    }
}

pub(super) fn mcp_cassette_decide(key: &str) -> Option<sema_core::McpCassetteDecision> {
    use crate::cassette::Decision;
    let scope = current_cassette_scope()?;
    let decision = scope.borrow().decide(key);
    Some(match decision {
        Decision::Replay(entry) => match entry.mcp_result {
            Some(value) => sema_core::McpCassetteDecision::Replay(value),
            // Present under this key but not an mcp-call entry — treat as drift.
            None => sema_core::McpCassetteDecision::Miss,
        },
        Decision::Miss(_) => sema_core::McpCassetteDecision::Miss,
        Decision::Record => sema_core::McpCassetteDecision::Record(
            sema_core::McpCassetteRecorder::new(scope, key.to_string()),
        ),
    })
}

/// Install a cassette in the current ambient LLM scope. Tasks spawned afterward
/// inherit it; the `SEMA_LLM_CASSETTE` path uses the same baseline.
pub fn install_cassette(cassette: crate::cassette::Cassette) {
    install_cassette_scope(Some(cassette_scope(cassette)));
}

/// Remove the cassette from the current ambient scope and return an owned
/// snapshot. Already-spawned tasks retain their shared scope and flush any later
/// recordings when its final owner is dropped.
pub fn take_cassette() -> Option<crate::cassette::Cassette> {
    let scope = install_cassette_scope(None)?;
    match Rc::try_unwrap(scope) {
        Ok(mut state) => state.cassette.get_mut().take(),
        Err(scope) => {
            let mut active = scope.borrow_mut();
            persist_cassette_off_quantum(&mut active);
            let cassette = active.clone();
            Some(cassette)
        }
    }
}

pub(super) fn register(env: &Env) {
    // (llm/with-cassette "path.jsonl" [{:mode :auto}] thunk). The tape LOAD is a disk
    // read; under the cooperative runtime it OFFLOADS to the blocking tier so the
    // quantum never touches the filesystem, then installs the scope and calls the body.
    env.set(
        sema_core::intern("llm/with-cassette"),
        Value::native_fn(NativeFn::with_ctx_runtime(
            "llm/with-cassette",
            |ctx, args| {
                // Host (non-quantum) path: synchronous load, body call, then flush.
                let (path, mode, body_fn) = parse_with_cassette_args(args)?;
                let cassette = crate::cassette::Cassette::load(path, mode);
                let teardown = install_loaded_cassette(cassette);
                let result = sema_core::call_callback(ctx, &body_fn, &[]);
                teardown();
                result
            },
            |_native_ctx, args| with_cassette_runtime(args),
        )),
    );

    // (llm/cassette-load "path" [{:mode :replay}]) — install in the current ambient
    // scope. Tasks spawned afterward inherit this cassette; tasks that already exist
    // retain the scope they captured at spawn time. The tape load offloads off the
    // quantum under the runtime.
    env.set(
        sema_core::intern("llm/cassette-load"),
        Value::native_fn(NativeFn::with_ctx_runtime(
            "llm/cassette-load",
            |_ctx, args| {
                let (path, mode) = parse_cassette_load_args(args)?;
                let cassette = crate::cassette::Cassette::load(path, mode);
                install_cassette(cassette);
                Ok(Value::nil())
            },
            |_native_ctx, args| cassette_load_runtime(args),
        )),
    );

    register_fn(env, "llm/cassette-save", |_args| {
        // Flush the active tape OFF the quantum (best-effort under the runtime; the
        // in-memory tape is authoritative, disk persistence is best-effort).
        match current_cassette_scope() {
            Some(scope) => {
                let mut guard = scope.borrow_mut();
                persist_cassette_off_quantum(&mut guard);
                Ok(Value::bool(true))
            }
            None => Ok(Value::bool(false)),
        }
    });

    register_fn(env, "llm/cassette-eject", |_args| {
        if let Some(mut cass) = take_cassette() {
            persist_cassette_off_quantum(&mut cass);
            Ok(Value::bool(true))
        } else {
            Ok(Value::bool(false))
        }
    });

    // --- Fallback provider builtins ---
}
