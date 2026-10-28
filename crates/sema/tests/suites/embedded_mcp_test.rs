use sema::mcp::{McpCallError, McpServerOptions, McpSession};
use sema::{Interpreter, Value};
use serde_json::json;

#[test]
fn public_session_calls_host_functions_and_preserves_definitions() {
    let interp = Interpreter::new();
    interp.register_fn("host/greet", |args| {
        Ok(Value::string(&format!(
            "hello {}",
            args[0].as_str().unwrap()
        )))
    });
    interp.eval_str(r#"(deftool greet "Greeting" {:name {:type :string}} (lambda (name) (host/greet name)))"#).unwrap();
    let session = McpSession::new(&interp, McpServerOptions::embedded()).unwrap();
    let names: Vec<_> = session
        .list_tools(&interp)
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(names.contains(&"greet".to_owned()));
    assert!(!names.contains(&"eval".to_owned()));
    let result = session
        .call_tool(&interp, "greet", &json!({"name":"Kameko"}))
        .unwrap();
    assert!(!result.is_error);
    assert!(serde_json::to_string(&result)
        .unwrap()
        .contains("hello Kameko"));
    assert!(matches!(
        session.call_tool(&interp, "eval", &json!({"code":"42"})),
        Err(McpCallError::NotAllowed(_))
    ));
}

#[test]
fn public_session_supports_async_host_functions_in_deftools() {
    let interp = Interpreter::new();
    interp.register_async_fn_typed("host/double", |n: i64| async move {
        Ok::<_, std::convert::Infallible>(n * 2)
    });
    interp
        .eval_str(r#"(deftool double "Double" {:n {:type :integer}} (lambda (n) (host/double n)))"#)
        .unwrap();
    let session = McpSession::new(&interp, McpServerOptions::embedded()).unwrap();
    let result = session
        .call_tool(&interp, "double", &json!({"n":21}))
        .unwrap();
    assert!(!result.is_error, "{result:?}");
    assert!(serde_json::to_string(&result).unwrap().contains("42"));
    assert_eq!(interp.runtime_live_task_count(), 0);
}

#[test]
fn session_handles_protocol_frames_without_owning_transport() {
    let interp = Interpreter::new();
    let session = McpSession::new(
        &interp,
        McpServerOptions::embedded().server_info("host", "1.2.3"),
    )
    .unwrap();
    let mut output = Vec::new();
    session
        .handle_message(
            &interp,
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            &mut output,
        )
        .unwrap();
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["result"]["serverInfo"]["name"], "host");
    assert_eq!(response["result"]["serverInfo"]["version"], "1.2.3");
    output.clear();
    session
        .handle_message(
            &interp,
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &mut output,
        )
        .unwrap();
    assert!(output.is_empty());
}

#[test]
fn protocol_rejects_invalid_requests_and_recovers() {
    let interp = Interpreter::new();
    let session = McpSession::new(&interp, McpServerOptions::embedded()).unwrap();
    for (frame, code) in [
        (b"{".as_slice(), -32700),
        (
            br#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#.as_slice(),
            -32600,
        ),
        (
            br#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#.as_slice(),
            -32600,
        ),
        (br#"{"jsonrpc":"2.0","id":1}"#.as_slice(), -32600),
    ] {
        let mut output = Vec::new();
        session.handle_message(&interp, frame, &mut output).unwrap();
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], code);
    }
    assert!(
        !session
            .call_tool(&interp, "info", &json!({}))
            .unwrap()
            .is_error
    );
}

#[test]
fn eval_requires_explicit_enablement_and_exclusion_wins() {
    let interp = Interpreter::new();
    assert!(McpSession::new(&interp, McpServerOptions::embedded().also_tools(["eval"])).is_err());
    let session = McpSession::new(
        &interp,
        McpServerOptions::embedded()
            .enable_eval()
            .exclude_tools(["eval"]),
    )
    .unwrap();
    assert!(!session.list_tools(&interp).iter().any(|t| t.name == "eval"));
    let session = McpSession::new(&interp, McpServerOptions::embedded().enable_eval()).unwrap();
    assert!(
        !session
            .call_tool(&interp, "eval", &json!({"code":"(define x 41) (+ x 1)"}))
            .unwrap()
            .is_error
    );
    assert_eq!(interp.eval_str("x").unwrap(), Value::int(41));
}

#[test]
fn timeout_cancels_a_parked_host_future_and_session_recovers() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let interp = Interpreter::new();
    interp.register_async_fn_typed("host/wait", move || {
        let flag = flag.clone();
        async move {
            let _drop = Dropped(flag);
            std::future::pending::<()>().await;
            Ok::<_, std::convert::Infallible>(0i64)
        }
    });
    let session = McpSession::new(
        &interp,
        McpServerOptions::embedded()
            .enable_eval()
            .timeout(Some(Duration::from_millis(100))),
    )
    .unwrap();
    let start = Instant::now();
    assert!(matches!(
        session.call_tool(
            &interp,
            "eval",
            &json!({"code":"(println \"before-timeout\") (host/wait)","timeout_ms":0})
        ),
        Err(McpCallError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(1);
    while !dropped.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(dropped.load(Ordering::SeqCst));
    let recovered = session
        .call_tool(&interp, "eval", &json!({"code":"42"}))
        .unwrap();
    assert!(!recovered.is_error);
    assert!(!serde_json::to_string(&recovered)
        .unwrap()
        .contains("before-timeout"));
    assert_eq!(interp.runtime_live_task_count(), 0);
}

#[test]
fn panicking_host_function_is_cancelled_and_session_recovers() {
    let interp = Interpreter::new();
    interp.register_fn("host/panic", |_| panic!("test host panic"));
    let session = McpSession::new(&interp, McpServerOptions::embedded().enable_eval()).unwrap();
    assert!(matches!(
        session.call_tool(&interp, "eval", &json!({"code":"(host/panic)"})),
        Err(McpCallError::Panicked(_))
    ));
    assert_eq!(interp.runtime_live_task_count(), 0);
    assert!(
        !session
            .call_tool(&interp, "eval", &json!({"code":"42"}))
            .unwrap()
            .is_error
    );
}

#[test]
fn deftool_collections_keep_the_json_text_representation() {
    let interp = Interpreter::new();
    interp
        .eval_str(r#"(deftool data "Data" {} (lambda () {:items '(1 2)}))"#)
        .unwrap();
    let session = McpSession::new(&interp, McpServerOptions::embedded()).unwrap();
    let result = session.call_tool(&interp, "data", &json!({})).unwrap();
    let text = match &result.content[0] {
        sema::mcp::ToolContent::Text { text } => text,
        _ => panic!("expected text content"),
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).unwrap(),
        json!({"items":[1,2]})
    );
}

#[test]
fn oversized_frame_is_drained_before_the_next_request() {
    let interp = Interpreter::new();
    let session =
        McpSession::new(&interp, McpServerOptions::embedded().max_frame_bytes(128)).unwrap();
    let mut input = vec![b'x'; 1000];
    input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
    let mut output = Vec::new();
    session
        .serve_on(&interp, std::io::Cursor::new(input), &mut output)
        .unwrap();
    let responses: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["error"]["code"], -32600);
    assert_eq!(responses[1]["id"], 2);
}

#[test]
fn output_is_bounded_and_host_hooks_are_restored() {
    use std::sync::{Arc, Mutex};
    let interp = Interpreter::new();
    let output = Arc::new(Mutex::new(String::new()));
    let sink = output.clone();
    let _scope = sema_core::output_hook::HostHookScope::new(
        Some(Box::new(move |s| sink.lock().unwrap().push_str(s))),
        None,
    );
    let session = McpSession::new(
        &interp,
        McpServerOptions::embedded()
            .enable_eval()
            .max_output_bytes(64),
    )
    .unwrap();
    let result = session
        .call_tool(
            &interp,
            "eval",
            &json!({"code":"(dotimes (i 10000) (print \"abcd\"))"}),
        )
        .unwrap();
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.len() < 512, "{}", text.len());
    assert!(text.contains("output truncated"));
    sema_core::write_stdout("after");
    assert_eq!(&*output.lock().unwrap(), "after");
}

#[test]
fn restricted_file_tools_check_caps_before_reading_or_writing() {
    use sema::{Caps, Sandbox};
    let interp = Interpreter::builder()
        .with_sandbox(Sandbox::deny(Caps::FS_READ.union(Caps::FS_WRITE)))
        .build();
    let session = McpSession::new(
        &interp,
        McpServerOptions::embedded().also_tools([
            "run_file",
            "compile",
            "fmt",
            "disasm",
            "notebook/read",
        ]),
    )
    .unwrap();
    for (name, args) in [
        ("run_file", json!({"file_path":"missing.sema"})),
        ("compile", json!({"source_path":"missing.sema"})),
        ("fmt", json!({"file_path":"missing.sema"})),
        ("disasm", json!({"file_path":"missing.sema"})),
        ("notebook/read", json!({"path":"missing.sema-nb"})),
    ] {
        let result = session.call_tool(&interp, name, &args).unwrap();
        assert!(result.is_error, "{name}");
        assert!(
            serde_json::to_string(&result).unwrap().contains("denied"),
            "{result:?}"
        );
    }
}

#[test]
fn nested_eval_returns_error_before_creating_a_root() {
    let interp = std::rc::Rc::new(Interpreter::new());
    let weak = std::rc::Rc::downgrade(&interp);
    interp.register_fn("host/nested", move |_| {
        weak.upgrade().unwrap().eval_str("42")
    });
    let session = McpSession::new(&interp, McpServerOptions::embedded().enable_eval()).unwrap();
    let result = session
        .call_tool(&interp, "eval", &json!({"code":"(host/nested)"}))
        .unwrap();
    assert!(result.is_error);
    assert!(serde_json::to_string(&result)
        .unwrap()
        .contains("nested evaluation"));
    assert_eq!(interp.runtime_live_task_count(), 0);
}

#[test]
fn nested_evaluation_of_a_different_interpreter_is_rejected() {
    let nested = std::rc::Rc::new(Interpreter::new());
    let interp = Interpreter::new();
    let weak = std::rc::Rc::downgrade(&nested);
    interp.register_fn("host/nested", move |_| {
        weak.upgrade().unwrap().eval_str("42")
    });
    let session = McpSession::new(&interp, McpServerOptions::embedded().enable_eval()).unwrap();
    let result = session
        .call_tool(&interp, "eval", &json!({"code":"(host/nested)"}))
        .unwrap();
    assert!(result.is_error);
    assert!(serde_json::to_string(&result)
        .unwrap()
        .contains("nested evaluation"));
    assert_eq!(nested.runtime_live_task_count(), 0);
    assert_eq!(interp.runtime_live_task_count(), 0);
}

#[tokio::test]
async fn worker_can_call_async_functions_and_shutdown_from_tokio() {
    let worker = sema::mcp::McpWorker::spawn(
        Interpreter::builder(),
        McpServerOptions::embedded(),
        |interp| {
            interp.register_async_fn_typed("host/double", |n: i64| async move {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                Ok::<_, std::convert::Infallible>(n * 2)
            });
            interp.eval_str(
                r#"(deftool double "Double" {:n {:type :integer}} (lambda (n) (host/double n)))"#,
            )?;
            Ok(())
        },
    )
    .unwrap();
    let handle = worker.handle();
    let result = handle.call_tool("double", json!({"n":21})).await.unwrap();
    assert!(!result.is_error, "{result:?}");
    assert!(serde_json::to_string(&result).unwrap().contains("42"));
    tokio::time::timeout(std::time::Duration::from_secs(3), worker.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(handle.call_tool("double", json!({"n":1})).await.is_err());
}

#[test]
fn hidden_tools_stay_hidden_and_source_limits_apply_before_reading() {
    let interp = Interpreter::new();
    interp
        .eval_str(r#"(deftool hidden "Hidden" {:mcp/expose #f} (lambda () 42))"#)
        .unwrap();
    let session =
        McpSession::new(&interp, McpServerOptions::embedded().only_tools(["hidden"])).unwrap();
    assert!(session.list_tools(&interp).is_empty());
    assert!(session.call_tool(&interp, "hidden", &json!({})).is_err());
    let dir = sema_core::testing::TempDir::new("embedded-mcp-large");
    let path = dir.path().join("large.sema");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(1 << 30)
        .unwrap();
    let session = McpSession::new(
        &interp,
        McpServerOptions::embedded().also_tools(["run_file"]),
    )
    .unwrap();
    let result = session
        .call_tool(&interp, "run_file", &json!({"file_path":path}))
        .unwrap();
    assert!(result.is_error);
    assert!(serde_json::to_string(&result)
        .unwrap()
        .contains("max_source_bytes"));
    let session = McpSession::new(
        &interp,
        McpServerOptions::embedded()
            .also_tools(["build"])
            .with_build_hook(|_, _| panic!("oversized input must not reach the build hook")),
    )
    .unwrap();
    let result = session
        .call_tool(
            &interp,
            "build",
            &json!({"source_path":path,"output_path":"unused"}),
        )
        .unwrap();
    assert!(result.is_error);
    assert!(serde_json::to_string(&result)
        .unwrap()
        .contains("max_source_bytes"));
}

#[test]
fn a_session_rejects_another_interpreter() {
    let one = Interpreter::new();
    let session = McpSession::new(&one, McpServerOptions::embedded()).unwrap();
    let two = Interpreter::new();
    assert!(matches!(
        session.call_tool(&two, "info", &json!({})),
        Err(McpCallError::InterpreterMismatch)
    ));
}

#[test]
fn opening_a_notebook_preserves_the_sessions_llm_provider() {
    let interpreter = Interpreter::new();
    let fake = sema_llm::fake::FakeProvider::builder("embedded-notebook")
        .reply("still configured")
        .build();
    let recorder = fake.recorder();
    sema_llm::builtins::register_test_provider(Box::new(fake));
    let session = McpSession::new(
        &interpreter,
        McpServerOptions::embedded().also_tools(["notebook/new", "notebook/read"]),
    )
    .unwrap();
    let dir = sema_core::testing::TempDir::new("embedded-notebook-provider");
    let path = dir.path().join("test.sema-nb");
    assert!(
        !session
            .call_tool(&interpreter, "notebook/new", &json!({"path":path}))
            .unwrap()
            .is_error
    );
    assert!(
        !session
            .call_tool(&interpreter, "notebook/read", &json!({"path":path}))
            .unwrap()
            .is_error
    );
    assert_eq!(
        interpreter
            .eval_str(r#"(llm/complete "hello")"#)
            .unwrap()
            .as_str(),
        Some("still configured")
    );
    assert_eq!(recorder.call_count(), 1);
}

#[tokio::test]
async fn worker_cancellation_targets_string_request_ids_and_recovers() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;
    let started = Arc::new(AtomicBool::new(false));
    let flag = started.clone();
    let worker = sema::mcp::McpWorker::spawn(
        Interpreter::builder(),
        McpServerOptions::embedded().enable_eval(),
        move |interp| {
            interp.register_async_fn_typed("host/wait", move || {
                let flag = flag.clone();
                async move {
                    flag.store(true, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                    Ok::<_, std::convert::Infallible>(0i64)
                }
            });
            Ok(())
        },
    )
    .unwrap();
    let handle = worker.handle();
    let call_handle = handle.clone();
    let call = tokio::spawn(async move {
        call_handle.handle_message(br#"{"jsonrpc":"2.0","id":"slow","method":"tools/call","params":{"name":"eval","arguments":{"code":"(host/wait)"}}}"#.to_vec()).await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.handle_message(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"slow"}}"#.to_vec()).await.unwrap();
    let output = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["id"], "slow");
    assert_eq!(response["result"]["isError"], true);
    assert!(
        !handle
            .call_tool("eval", json!({"code":"42"}))
            .await
            .unwrap()
            .is_error
    );
    worker.shutdown().await.unwrap();
}
