//! Nomic protocol tests use a synthetic response from the published API schema.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::thread;
use std::time::Duration;

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;
use sema_llm::nomic::NomicEmbeddingProvider;
use sema_llm::provider::LlmProvider;
use sema_llm::types::{EmbedOptions, EmbedRequest, LlmError};
use serde_json::{json, Value};

const RESPONSE: &str = include_str!("../fixtures/nomic/embed-response.json");

struct Server {
    url: String,
    requests: mpsc::Receiver<(String, Vec<String>, Value)>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new(status: u16, response: String, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (tx, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("accept: {e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    continue;
                }
                let request_line = line.trim().to_string();
                let mut headers = Vec::new();
                let mut size = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            size = value.trim().parse().unwrap();
                        }
                    }
                    headers.push(line.trim().to_ascii_lowercase());
                }
                let mut bytes = vec![0; size];
                reader.read_exact(&mut bytes).unwrap();
                tx.send((
                    request_line,
                    headers,
                    serde_json::from_slice(&bytes).unwrap(),
                ))
                .unwrap();
                thread::sleep(delay);
                let _ = write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nRetry-After: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len());
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn fixture() -> Self {
        Self::new(200, RESPONSE.to_string(), Duration::ZERO)
    }

    fn interpreter(&self, options: &str) -> Interpreter {
        let interp = Interpreter::new();
        reset_runtime_state();
        interp.eval_str_compiled(&format!(r#"(llm/configure-embeddings :nomic {{:api-key "test-key" :base-url "{}/v1/" {options}}})"#, self.url)).unwrap();
        interp
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
        let result = self.thread.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
fn nomic_uses_native_schema_and_preserves_vector_order() {
    for expression in [
        r#"(map embedding/->list (llm/embed ["first" "second"]))"#,
        r#"(map embedding/->list (async/await (async/spawn (fn () (llm/embed ["first" "second"])))))"#,
    ] {
        let server = Server::fixture();
        let interp = server.interpreter(":dimensions 3 :task \"search_document\" :long-text-mode \"mean\" :max-tokens-per-text 4096");
        let result = interp.eval_str_compiled(expression).unwrap();
        assert_eq!(result.to_string(), "((0.25 0.75 -0.5) (-0.5 0.125 0.875))");
        let (line, headers, body) = server
            .requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(line, "POST /v1/embedding/text HTTP/1.1");
        assert!(headers.contains(&"authorization: bearer test-key".to_string()));
        assert!(headers.contains(&"content-type: application/json".to_string()));
        assert_eq!(
            body,
            json!({"texts":["first","second"], "model":"nomic-embed-text-v1.5", "task_type":"search_document", "dimensionality":3, "long_text_mode":"mean", "max_tokens_per_text":4096})
        );
    }
}

#[test]
fn nomic_per_call_options_override_configuration() {
    let server = Server::fixture();
    let interp = server.interpreter(
        ":dimensions 8 :task \"clustering\" :long-text-mode \"mean\" :max-tokens-per-text 8192",
    );
    interp.eval_str_compiled(r#"(llm/embed ["first" "second"] {:dimensions 3 :task "search_query" :long-text-mode "truncate" :max-tokens-per-text 1024})"#).unwrap();
    let (_, _, body) = server
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(body["task_type"], "search_query");
    assert_eq!(body["dimensionality"], 3);
    assert_eq!(body["long_text_mode"], "truncate");
    assert_eq!(body["max_tokens_per_text"], 1024);
}

#[test]
fn nomic_single_input_model_override_and_sync_usage() {
    let mut value: Value = serde_json::from_str(RESPONSE).unwrap();
    value["embeddings"] = json!([[0.25, 0.75, -0.5]]);
    let server = Server::new(200, value.to_string(), Duration::ZERO);
    let interp = server.interpreter(":model \"nomic-embed-text-v1\"");
    let value = interp
        .eval_str_compiled(
            r#"(embedding/->list (llm/embed "hello" {:model "nomic-embed-text-v1.5"}))"#,
        )
        .unwrap();
    assert_eq!(value.to_string(), "(0.25 0.75 -0.5)");
    let (_, _, body) = server
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(body["texts"], json!(["hello"]));
    assert_eq!(body["model"], "nomic-embed-text-v1.5");

    let provider = NomicEmbeddingProvider::new(
        "test-key".into(),
        format!("{}/v1", server.url),
        "nomic-embed-text-v1.5".into(),
        EmbedOptions::default(),
    )
    .unwrap();
    let response = provider
        .embed(EmbedRequest {
            texts: vec!["hello".into()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(response.usage.prompt_tokens, 7);
    assert_eq!(response.usage.total_tokens(), 7);
}

#[test]
fn nomic_rate_limit_retains_retry_hint() {
    let server = Server::new(429, "{}".into(), Duration::ZERO);
    let provider = NomicEmbeddingProvider::new(
        "test-key".into(),
        format!("{}/v1", server.url),
        "nomic-embed-text-v1.5".into(),
        EmbedOptions::default(),
    )
    .unwrap();
    assert!(matches!(
        provider.embed(EmbedRequest {
            texts: vec!["hello".into()],
            ..Default::default()
        }),
        Err(LlmError::RateLimited {
            retry_after_ms: 1000
        })
    ));
}

#[test]
fn nomic_configuration_preserves_chat_and_forwards_options_to_custom_providers() {
    let server = Server::fixture();
    let interp = server.interpreter("");
    let fake = FakeProvider::builder("fake")
        .reply("chat stays configured")
        .embed(vec![vec![1.0]])
        .build();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    interp
        .eval_str_compiled(&format!(
            r#"(llm/configure-embeddings :nomic {{:api-key "test-key" :base-url "{}/v1"}})"#,
            server.url
        ))
        .unwrap();
    assert_eq!(
        interp
            .eval_str_compiled(r#"(llm/complete "hello")"#)
            .unwrap()
            .as_str(),
        Some("chat stays configured")
    );
    interp.eval_str_compiled(r#"(llm/embed "hello" {:provider :fake :long-text-mode "mean" :max-tokens-per-text 4096})"#).unwrap();
    let requests = recorder.embeds();
    assert_eq!(requests[0].options.long_text_mode.as_deref(), Some("mean"));
    assert_eq!(requests[0].options.max_tokens_per_text, Some(4096));
    assert!(server.requests.try_recv().is_err());
}

#[test]
fn nomic_cassettes_distinguish_long_text_options() {
    let server = Server::fixture();
    let interp = server.interpreter(":dimensions 3");
    let tape = std::env::temp_dir().join(format!(
        "sema-nomic-cassette-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = serde_json::to_string(&tape).unwrap();
    interp.eval_str_compiled(&format!(r#"(llm/with-cassette {path} {{:mode :record}} (fn () (llm/embed ["first" "second"] {{:long-text-mode "truncate" :max-tokens-per-text 1024}})))"#)).unwrap();
    server
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    interp.eval_str_compiled(&format!(r#"(llm/with-cassette {path} {{:mode :replay}} (fn () (llm/embed ["first" "second"] {{:long-text-mode "truncate" :max-tokens-per-text 1024}})))"#)).unwrap();
    for options in [
        r#":long-text-mode "mean" :max-tokens-per-text 1024"#,
        r#":long-text-mode "truncate" :max-tokens-per-text 2048"#,
    ] {
        let error = interp.eval_str_compiled(&format!(r#"(llm/with-cassette {path} {{:mode :replay}} (fn () (llm/embed ["first" "second"] {{{options}}})))"#)).unwrap_err().to_string();
        assert!(error.contains("cassette"), "{error}");
    }
    assert!(server.requests.try_recv().is_err());
    std::fs::remove_file(tape).unwrap();
}

#[test]
fn nomic_auto_configuration_uses_key_and_endpoint_override() {
    for forced in [false, true] {
        let server = Server::fixture();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_sema"));
        command
            .env_clear()
            .env("NOMIC_API_KEY", "test-key")
            .env("NOMIC_BASE_URL", format!("{}/v1", server.url));
        if forced {
            command
                .env("SEMA_EMBEDDING_PROVIDER", "nomic")
                .env("SEMA_EMBEDDING_MODEL", "nomic-embed-text-v1.5");
        }
        let output = command
            .args([
                "-e",
                r#"(map embedding/length (llm/embed ["first" "second"] {:dimensions 3}))"#,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "(3 3)");
        let (_, headers, body) = server
            .requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(headers.contains(&"authorization: bearer test-key".to_string()));
        assert_eq!(body["model"], "nomic-embed-text-v1.5");
    }
}

#[test]
fn nomic_rejects_malformed_responses() {
    let fixture: Value = serde_json::from_str(RESPONSE).unwrap();
    let mut invalid = Vec::new();
    for field in ["model", "usage", "embeddings"] {
        let mut value = fixture.clone();
        value.as_object_mut().unwrap().remove(field);
        invalid.push(value);
    }
    for (pointer, replacement) in [
        ("/embeddings/0/1", json!("bad")),
        ("/embeddings/0", json!([])),
        ("/embeddings/0", json!([1])),
        ("/embeddings", json!([[1, 2, 3]])),
        ("/usage/prompt_tokens", json!(-1)),
        ("/usage/total_tokens", json!(1)),
        ("/usage/prompt_tokens", json!(4294967296_u64)),
        ("/model", json!("wrong-model")),
    ] {
        let mut value = fixture.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        invalid.push(value);
    }
    for value in invalid {
        let server = Server::new(200, value.to_string(), Duration::ZERO);
        let interp = server.interpreter(":dimensions 3");
        assert!(
            interp
                .eval_str_compiled(r#"(llm/embed ["first" "second"])"#)
                .is_err(),
            "accepted {value}"
        );
    }
}

#[test]
fn nomic_errors_and_timeout_reach_the_caller() {
    for status in [401, 403, 422] {
        let server = Server::new(
            status,
            r#"{"detail":"access or input rejected"}"#.to_string(),
            Duration::ZERO,
        );
        let interp = server.interpreter("");
        let error = interp
            .eval_str_compiled(r#"(llm/embed "hello")"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&status.to_string()), "{error}");
    }
    let server = Server::new(200, RESPONSE.to_string(), Duration::from_millis(150));
    let interp = server.interpreter("");
    assert!(interp
        .eval_str_compiled(r#"(llm/embed ["first" "second"] {:timeout 20})"#)
        .is_err());
}

#[test]
fn nomic_invalid_options_empty_batch_and_rerank_do_not_dispatch() {
    let server = Server::fixture();
    let interp = server.interpreter("");
    for options in [
        r#":task "wrong""#,
        r#":long-text-mode "wrong""#,
        ":max-tokens-per-text 0",
        ":max-tokens-per-text 4294967296",
        ":truncate #f",
        ":keep-alive 0",
        r#":input-type "query""#,
    ] {
        assert!(
            interp
                .eval_str_compiled(&format!(r#"(llm/embed "hello" {{{options}}})"#))
                .is_err(),
            "{options}"
        );
    }
    assert_eq!(
        interp
            .eval_str_compiled("(llm/embed [])")
            .unwrap()
            .to_string(),
        "()"
    );
    let error = interp
        .eval_str_compiled(r#"(llm/rerank "query" ["doc"] {:provider :nomic})"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not support reranking"), "{error}");
    assert!(server.requests.try_recv().is_err());
}

#[test]
#[ignore = "release gate: requires approved NOMIC_API_KEY and real hosted API access"]
fn live_nomic_embedding_release_gate() {
    let key = std::env::var("NOMIC_API_KEY")
        .expect("approved NOMIC_API_KEY is required; this gate must not skip");
    assert!(!key.trim().is_empty(), "NOMIC_API_KEY must not be empty");
    let interp = Interpreter::new();
    reset_runtime_state();
    interp
        .eval_str_compiled(&format!(
            r#"(llm/configure-embeddings :nomic {{:api-key {} :timeout 30000}})"#,
            serde_json::to_string(&key).unwrap()
        ))
        .unwrap();
    let result = interp.eval_str_compiled(r#"
        (define docs (llm/embed ["Cats are mammals." "Rocks are minerals."] {:task "search_document"}))
        (define query (llm/embed "Which animals are mammals?" {:task "search_query"}))
        (define reduced (async/await (async/spawn (fn () (llm/embed "Cats are mammals." {:dimensions 256 :task "classification" :long-text-mode "mean" :max-tokens-per-text 1024})))))
        (list (length docs) (map embedding/length docs) (embedding/length query) (embedding/length reduced)
              (> (llm/similarity query (first docs)) (llm/similarity query (second docs))))
    "#).unwrap();
    assert_eq!(result.to_string(), "(2 (768 768) 768 256 #t)");
}
