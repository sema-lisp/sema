//! Request-level regressions: exercise the real providers through the interpreter.
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
use serde_json::{json, Value};

struct Server {
    url: String,
    requests: mpsc::Receiver<(String, Value)>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        Self::delayed(Duration::ZERO)
    }

    fn delayed(delay: Duration) -> Self {
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
                let path = line.split_whitespace().nth(1).unwrap().to_string();
                let mut size = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            size = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut bytes = vec![0; size];
                reader.read_exact(&mut bytes).unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                let count = body["input"]
                    .as_array()
                    .or_else(|| body["texts"].as_array())
                    .map_or(1, Vec::len);
                let vectors = vec![vec![0.25, 0.75]; count];
                let response = json!({
                    "model": body["model"],
                    "data": if path.ends_with("/rerank") { json!([{"index":1,"relevance_score":0.9}]) } else { json!(vectors.iter().enumerate().map(|(index, embedding)| json!({"index":index,"embedding":embedding})).collect::<Vec<_>>()) },
                    "embeddings": if path.ends_with("/api/embed") { json!(vectors) } else { json!({"float":vectors}) },
                    "usage": {"prompt_tokens": 7}, "prompt_eval_count": 7,
                    "choices": [{"message":{"role":"assistant","content":"wire chat"},"finish_reason":"stop"}],
                    "results": [{"index":1,"relevance_score":0.9}]
                }).to_string();
                tx.send((path, body)).unwrap();
                thread::sleep(delay);
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response);
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn request(&self) -> (String, Value) {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .expect("captured request")
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
fn interpreter() -> Interpreter {
    let interp = Interpreter::new();
    reset_runtime_state();
    interp
}

#[test]
fn configured_openai_embedding_model_and_per_call_override() {
    for key in ["model", "default-model"] {
        let server = Server::new();
        let interp = interpreter();
        interp.eval_str_compiled(&format!(r#"
            (llm/configure-embeddings :openai {{:api-key "test" :base-url "{}" :{key} "configured-embedding"}})
            (embedding/length (llm/embed "hello"))
        "#, server.url)).unwrap();
        assert_eq!(server.request().1["model"], "configured-embedding");
        interp
            .eval_str_compiled(r#"(llm/embed "hello" {:model "override-embedding"})"#)
            .unwrap();
        assert_eq!(server.request().1["model"], "override-embedding");
    }
}

#[test]
fn embedding_configuration_preserves_openai_chat() {
    let server = Server::new();
    let interp = interpreter();
    let fake = FakeProvider::builder("openai")
        .model("chat-model")
        .reply("fake chat")
        .build();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(&format!(r#"
        (llm/configure-embeddings :openai {{:api-key "test" :base-url "{}" :model "embedding-model"}})
        (llm/complete "hello")
    "#, server.url)).unwrap();
    assert_eq!(result.as_str(), Some("fake chat"));
    assert_eq!(recorder.call_count(), 1);
}

#[test]
fn ollama_embeddings_use_native_endpoint_and_separate_model() {
    let server = Server::new();
    let interp = interpreter();
    let result = interp
        .eval_str_compiled(&format!(
            r#"
        (llm/configure-embeddings :ollama {{:base-url "{}" :model "embeddinggemma"}})
        (embedding/length (llm/embed "hello"))
    "#,
            server.url
        ))
        .unwrap();
    assert_eq!(result.as_int(), Some(2));
    let (path, body) = server.request();
    assert_eq!(path, "/api/embed");
    assert_eq!(body["model"], "embeddinggemma");
}

#[test]
fn embedding_provider_override_does_not_change_default() {
    let server = Server::new();
    let interp = interpreter();
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .embed(vec![vec![1.0]])
        .build();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(&format!(r#"
        (llm/configure-embeddings :openai {{:api-key "test" :base-url "{}" :model "embedding-model"}})
        (embedding/length (llm/embed "hello" {{:provider :fake}}))
    "#, server.url)).unwrap();
    assert_eq!(result.as_int(), Some(1));
    assert_eq!(recorder.embeds().len(), 1);
    interp.eval_str_compiled(r#"(llm/embed "hello")"#).unwrap();
    assert_eq!(server.request().1["model"], "embedding-model");
}

#[test]
fn embedding_options_and_inputs_are_validated_before_dispatch() {
    for source in [
        r#"(llm/embed "hello" 3)"#,
        r#"(llm/embed ["hello" 3])"#,
        r#"(llm/embed "hello" {:model 3})"#,
        r#"(llm/embed "hello" {:provider 3})"#,
    ] {
        let interp = interpreter();
        let fake = FakeProvider::builder("fake").embed(vec![vec![1.0]]).build();
        let recorder = fake.recorder();
        register_test_provider(Box::new(fake));
        assert!(interp.eval_str_compiled(source).is_err(), "{source}");
        assert!(
            recorder.embeds().is_empty(),
            "invalid input reached provider: {source}"
        );
    }
}

#[test]
fn configured_embedding_options_can_be_overridden_per_call() {
    for (provider, field, configured, overridden) in [
        ("openai", "dimensions", json!(64), json!(32)),
        (
            "cohere",
            "input_type",
            json!("search_document"),
            json!("search_query"),
        ),
        ("ollama", "truncate", json!(true), json!(false)),
    ] {
        let server = Server::new();
        let interp = interpreter();
        let (option, first, second) = match provider {
            "openai" => ("dimensions", "64", "32"),
            "cohere" => ("input-type", "\"search_document\"", "\"search_query\""),
            _ => ("truncate", "#t", "#f"),
        };
        interp.eval_str_compiled(&format!(r#"
            (llm/configure-embeddings :{provider} {{:api-key "test" :base-url "{}" :model "embedding-model" :{option} {first}}})
            (llm/embed "hello")
            (llm/embed "hello" {{:{option} {second}}})
        "#, server.url)).unwrap();
        assert_eq!(
            server.request().1[field],
            configured,
            "{provider} configured option"
        );
        assert_eq!(
            server.request().1[field],
            overridden,
            "{provider} per-call option"
        );
    }
}

#[test]
fn auto_configuration_honors_forced_openai_with_other_keys_present() {
    let server = Server::new();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .env_clear()
        .env("OPENAI_API_KEY", "test")
        .env("JINA_API_KEY", "test")
        .env("OPENAI_BASE_URL", &server.url)
        .env("SEMA_EMBEDDING_PROVIDER", "openai")
        .env("SEMA_EMBEDDING_MODEL", "selected-embedding")
        .args(["-e", r#"(embedding/length (llm/embed "hello"))"#])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.request().1["model"], "selected-embedding");
}

#[test]
fn all_hosted_embedding_providers_honor_url_models_and_batch_input() {
    for provider in [
        "openai",
        "jina",
        "voyage",
        "cohere",
        "mistral",
        "gemini",
        "together",
        "fireworks",
        "custom",
    ] {
        let server = Server::new();
        let interp = interpreter();
        for configure in ["llm/configure-embeddings", "llm/configure"] {
            if configure == "llm/configure" && !matches!(provider, "jina" | "voyage" | "cohere") {
                continue;
            }
            let result = interp.eval_str_compiled(&format!(r#"
                ({configure} :{provider} {{:api-key "test" :base-url "{}/custom/" :model "alias-model" :default-model "configured-model"}})
                (map embedding/length (llm/embed ["first" "second"] {{:provider "{provider}"}}))
            "#, server.url)).unwrap();
            assert_eq!(result.to_string(), "(2 2)", "{provider}");
            let (path, request) = server.request();
            assert_eq!(
                path,
                if provider == "cohere" {
                    "/custom/embed"
                } else {
                    "/custom/embeddings"
                }
            );
            assert_eq!(request["model"], "configured-model", "{provider}");
            assert_eq!(
                request[if provider == "cohere" {
                    "texts"
                } else {
                    "input"
                }],
                json!(["first", "second"])
            );
            interp
                .eval_str_compiled(&format!(
                    r#"(llm/embed "override" {{:provider :{provider} :model "per-call"}})"#
                ))
                .unwrap();
            assert_eq!(server.request().1["model"], "per-call");
        }
    }
}

#[test]
fn rerank_models_are_independent_and_overridable() {
    for provider in ["jina", "voyage", "cohere", "together", "fireworks"] {
        let server = Server::new();
        let interp = interpreter();
        let result = interp.eval_str_compiled(&format!(r#"
            (llm/configure-embeddings :{provider} {{:api-key "test" :base-url "{}/custom/" :model "embed-model" :rerank-model "configured-reranker"}})
            (llm/rerank "query" ["first" "second"] {{:top-k 1}})
        "#, server.url)).unwrap();
        assert!(result.to_string().contains("second"));
        let (path, request) = server.request();
        assert_eq!(path, "/custom/rerank");
        assert_eq!(request["model"], "configured-reranker");
        assert_eq!(
            request[if provider == "voyage" {
                "top_k"
            } else {
                "top_n"
            }],
            1
        );
        interp.eval_str_compiled(&format!(r#"(llm/rerank "query" ["first" "second"] {{:provider :{provider} :model "per-call-reranker"}})"#)).unwrap();
        assert_eq!(server.request().1["model"], "per-call-reranker");
        interp.eval_str_compiled(r#"(llm/embed "hello")"#).unwrap();
        assert_eq!(server.request().1["model"], "embed-model");
    }
}

#[test]
fn ollama_default_model_host_alias_and_url_normalization() {
    for key in ["base-url", "host"] {
        for bare in [true, false] {
            let server = Server::new();
            let interp = interpreter();
            let host = if bare {
                server.url.trim_start_matches("http://")
            } else {
                &server.url
            };
            interp
                .eval_str_compiled(&format!(
                    r#"
                (llm/configure-embeddings :ollama {{:{key} "{host}/"}})
                (llm/embed "hello")
                (llm/embed "hello" {{:model "custom-ollama-model"}})
            "#
                ))
                .unwrap();
            let (path, request) = server.request();
            assert_eq!(path, "/api/embed");
            assert_eq!(request["model"], "embeddinggemma");
            assert_eq!(server.request().1["model"], "custom-ollama-model");
        }
    }
}

#[test]
fn fake_provider_receives_resolved_model_and_embedding_options() {
    let interp = interpreter();
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .embed(vec![vec![1.0]])
        .build();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    interp
        .eval_str_compiled(
            r#"(llm/embed "hello" {:dimensions 32 :input-type "query" :truncate #f :timeout 500})"#,
        )
        .unwrap();
    let requests = recorder.embeds();
    assert_eq!(requests[0].model.as_deref(), Some("fake-model"));
    assert_eq!(requests[0].options.dimensions, Some(32));
    assert_eq!(requests[0].options.input_type.as_deref(), Some("query"));
    assert_eq!(requests[0].options.truncate, Some(false));
    assert_eq!(requests[0].options.timeout_ms, Some(500));
}

#[test]
fn rerank_top_k_default_is_overridable() {
    let server = Server::new();
    let interp = interpreter();
    interp
        .eval_str_compiled(&format!(
            r#"
        (llm/configure-embeddings :jina {{:api-key "test" :base-url "{}" :top-k 2}})
        (llm/rerank "query" ["first" "second"])
        (llm/rerank "query" ["first" "second"] {{:top-k 1}})
    "#,
            server.url
        ))
        .unwrap();
    assert_eq!(server.request().1["top_n"], 2);
    assert_eq!(server.request().1["top_n"], 1);
}

fn live_embedding(provider: &str, key: Option<&str>, model: &str, dimensions: i64) {
    if key.is_some_and(|key| std::env::var(key).map_or(true, |value| value.is_empty())) {
        eprintln!("skipping {provider}: API key is absent");
        return;
    }
    let interp = interpreter();
    let result = interp
        .eval_str_compiled(&format!(
            r#"
        (llm/configure-embeddings :{provider} {{:timeout 30000}})
        (define vectors (llm/embed ["A cat sleeps on the sofa." "A cat sleeps on the sofa."]))
        (list (map embedding/length vectors) (llm/similarity (first vectors) (car (cdr vectors))))
    "#
        ))
        .unwrap();
    let result = result.as_list().unwrap();
    assert_eq!(
        result[0].to_string(),
        format!("({dimensions} {dimensions})")
    );
    assert!(result[1].as_float().unwrap() > 0.999);
    let result = interp.eval_str_compiled(&format!(r#"
        (llm/configure-embeddings :{provider} {{:model "invalid-default-for-override-test" :timeout 30000}})
        (embedding/length (llm/embed "A cat sleeps on the sofa." {{:model "{model}"}}))
    "#)).unwrap();
    assert_eq!(result.as_int(), Some(dimensions));
}

macro_rules! live_embeddings {
    ($($test:ident: $provider:literal, $key:expr, $model:literal, $dims:literal;)*) => {$ (
        #[test]
        #[ignore = "requires provider credentials or a running local Ollama with embeddinggemma"]
        fn $test() { live_embedding($provider, $key, $model, $dims); }
    )*};
}
live_embeddings! {
    live_embedding_openai: "openai", Some("OPENAI_API_KEY"), "text-embedding-3-small", 1536;
    live_embedding_jina: "jina", Some("JINA_API_KEY"), "jina-embeddings-v3", 1024;
    live_embedding_voyage: "voyage", Some("VOYAGE_API_KEY"), "voyage-3", 1024;
    live_embedding_cohere: "cohere", Some("COHERE_API_KEY"), "embed-english-v3.0", 1024;
    live_embedding_mistral: "mistral", Some("MISTRAL_API_KEY"), "mistral-embed", 1024;
    live_embedding_gemini: "gemini", Some("GEMINI_API_KEY"), "gemini-embedding-001", 3072;
    live_embedding_ollama: "ollama", None, "embeddinggemma", 768;
    live_embedding_fireworks: "fireworks", Some("FIREWORKS_API_KEY"), "fireworks/qwen3-embedding-8b", 4096;
}

fn live_rerank(provider: &str, key: &str, model: &str) {
    if std::env::var(key).map_or(true, |v| v.is_empty()) {
        eprintln!("skipping {provider}: API key is absent");
        return;
    }
    let interp = interpreter();
    for options in [String::new(), format!(r#":rerank-model "{model}""#)] {
        let result = interp.eval_str_compiled(&format!(r#"
            (llm/configure-embeddings :{provider} {{:model "embedding-model-unused-by-rerank" {options}}})
            (llm/rerank "Where is the cat?" ["A cat sleeps on the sofa." "The weather is sunny."] {{:top-k 1 :provider :{provider}}})
        "#)).unwrap();
        let rows = result.as_list().unwrap();
        assert_eq!(rows.len(), 1);
        let row = rows[0].as_map_rc().unwrap();
        assert_eq!(row[&sema_core::Value::keyword("index")].as_int(), Some(0));
        assert!(row[&sema_core::Value::keyword("score")]
            .as_float()
            .unwrap()
            .is_finite());
    }
    interp
        .eval_str_compiled(&format!(
            r#"
        (llm/configure-embeddings :{provider} {{:rerank-model "invalid-default-for-override-test"}})
        (llm/rerank "cat" ["cat" "weather"] {{:model "{model}" :top-k 1}})
    "#
        ))
        .unwrap();
}

macro_rules! live_rerankers {
    ($($test:ident: $provider:literal, $key:literal, $model:literal;)*) => {$ (
        #[test]
        #[ignore = "requires provider credentials"]
        fn $test() { live_rerank($provider, $key, $model); }
    )*};
}
live_rerankers! {
    live_rerank_jina: "jina", "JINA_API_KEY", "jina-reranker-v2-base-multilingual";
    live_rerank_voyage: "voyage", "VOYAGE_API_KEY", "rerank-2.5";
    live_rerank_cohere: "cohere", "COHERE_API_KEY", "rerank-v3.5";
    live_rerank_fireworks: "fireworks", "FIREWORKS_API_KEY", "fireworks/qwen3-reranker-8b";
}

#[test]
#[ignore = "requires OPENAI_API_KEY"]
fn live_openai_large_model_and_dimensions() {
    if std::env::var("OPENAI_API_KEY").is_err() {
        return;
    }
    let interp = interpreter();
    let result = interp.eval_str_compiled(r#"
        (llm/configure-embeddings :openai {:model "text-embedding-3-large" :dimensions 512})
        (list (embedding/length (llm/embed "hello"))
              (embedding/length (llm/embed "hello" {:model "text-embedding-3-small" :dimensions 256})))
    "#).unwrap();
    assert_eq!(result.to_string(), "(512 256)");
}

#[test]
#[ignore = "requires local Ollama with embeddinggemma and bge-large"]
fn live_ollama_issue_169_model_override() {
    let interp = interpreter();
    let result = interp.eval_str_compiled(r#"
        (llm/configure-embeddings :ollama {:model "bge-large:latest" :keep-alive 0})
        (list (embedding/length (llm/embed "hello world"))
              (embedding/length (llm/embed "hello world" {:model "embeddinggemma" :dimensions 256 :truncate #f})))
    "#).unwrap();
    assert_eq!(result.to_string(), "(1024 256)");
}

#[test]
fn request_timeouts_override_configuration_for_embedding_and_rerank() {
    for operation in [
        r#"(llm/embed "hello" OPTIONS)"#,
        r#"(llm/rerank "query" ["first" "second"] OPTIONS)"#,
    ] {
        let server = Server::delayed(Duration::from_millis(120));
        let interp = interpreter();
        interp.eval_str_compiled(&format!(r#"(llm/configure-embeddings :jina {{:base-url "{}" :api-key "test" :timeout 20}})"#, server.url)).unwrap();
        assert!(interp
            .eval_str_compiled(&operation.replace("OPTIONS", "{}"))
            .is_err());
        server.request();
        interp
            .eval_str_compiled(&operation.replace("OPTIONS", "{:timeout 1000}"))
            .unwrap();
        server.request();
    }
}

#[test]
fn spawned_embedding_and_rerank_keep_call_overrides() {
    let server = Server::new();
    let interp = interpreter();
    interp.eval_str_compiled(&format!(r#"
        (llm/configure-embeddings :jina {{:api-key "test" :base-url "{}" :model "configured"}})
        (async/await (async/spawn (fn () (llm/embed "hello" {{:model "async-embed" :dimensions 32}}))))
        (async/await (async/spawn (fn () (llm/rerank "query" ["first" "second"] {{:model "async-rerank" :top-k 1}}))))
    "#, server.url)).unwrap();
    let (_, request) = server.request();
    assert_eq!(request["model"], "async-embed");
    assert_eq!(request["dimensions"], 32);
    let (_, request) = server.request();
    assert_eq!(request["model"], "async-rerank");
    assert_eq!(request["top_n"], 1);
}

#[test]
fn embedding_cassette_does_not_mix_providers_or_dimensions() {
    for changed_provider in [true, false] {
        let path = std::env::temp_dir().join(format!(
            "sema-embed-config-{}-{changed_provider}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let server = Server::new();
        let interp = interpreter();
        for (provider, dimension) in [
            ("first", 32),
            (
                if changed_provider { "second" } else { "first" },
                if changed_provider { 32 } else { 64 },
            ),
        ] {
            interp.eval_str_compiled(&format!(r#"
                (llm/configure-embeddings :{provider} {{:api-key "test" :base-url "{}" :model "same-model"}})
                (llm/with-cassette "{}" {{:mode :auto}} (fn () (llm/embed "same text" {{:dimensions {dimension}}})))
            "#, server.url, path.display())).unwrap();
        }
        server.request();
        let second = server.requests.try_recv();
        let _ = std::fs::remove_file(&path);
        assert!(
            second.is_ok(),
            "different configuration replayed a cached vector"
        );
    }
}

#[test]
fn provider_specific_options_and_defaults_reach_the_wire() {
    for (provider, option, first, second, field, expected_first, expected_second) in [
        (
            "jina",
            "task",
            "\"retrieval.passage\"",
            "\"retrieval.query\"",
            "task",
            json!("retrieval.passage"),
            json!("retrieval.query"),
        ),
        (
            "voyage",
            "dimensions",
            "1024",
            "512",
            "output_dimension",
            json!(1024),
            json!(512),
        ),
        (
            "voyage",
            "truncate",
            "#t",
            "#f",
            "truncation",
            json!(true),
            json!(false),
        ),
        (
            "cohere",
            "truncate",
            "#t",
            "#f",
            "truncate",
            json!("END"),
            json!("NONE"),
        ),
        (
            "cohere",
            "dimensions",
            "1024",
            "512",
            "output_dimension",
            json!(1024),
            json!(512),
        ),
        (
            "ollama",
            "keep-alive",
            "\"5m\"",
            "0",
            "keep_alive",
            json!("5m"),
            json!(0),
        ),
    ] {
        let server = Server::new();
        interpreter().eval_str_compiled(&format!(r#"
            (llm/configure-embeddings :{provider} {{:api-key "test" :base-url "{}" :{option} {first}}})
            (llm/embed "hello")
            (llm/embed "hello" {{:{option} {second}}})
        "#, server.url)).unwrap();
        assert_eq!(
            server.request().1[field],
            expected_first,
            "{provider}/{option}"
        );
        assert_eq!(
            server.request().1[field],
            expected_second,
            "{provider}/{option}"
        );
    }
    for (provider, model) in [
        ("jina", "jina-reranker-v2-base-multilingual"),
        ("voyage", "rerank-2.5"),
        ("cohere", "rerank-v3.5"),
        ("together", "mixedbread-ai/mxbai-rerank-large-v2"),
        ("fireworks", "fireworks/qwen3-reranker-8b"),
    ] {
        let server = Server::new();
        interpreter()
            .eval_str_compiled(&format!(
                r#"
            (llm/configure-embeddings :{provider} {{:api-key "test" :base-url "{}"}})
            (llm/rerank "query" ["first" "second"])
        "#,
                server.url
            ))
            .unwrap();
        assert_eq!(server.request().1["model"], model);
    }
}

#[test]
fn cli_overrides_environment_for_embedding_and_rerank_independently() {
    let server = Server::new();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .env_clear()
        .env("OPENAI_API_KEY", "test")
        .env("JINA_API_KEY", "test")
        .env("JINA_BASE_URL", &server.url)
        .env("OPENAI_BASE_URL", &server.url)
        .env("SEMA_EMBEDDING_PROVIDER", "jina")
        .env("SEMA_EMBEDDING_MODEL", "wrong-embed")
        .env("SEMA_RERANK_PROVIDER", "cohere")
        .env("SEMA_RERANK_MODEL", "wrong-rerank")
        .args([
            "--embedding-provider",
            "openai",
            "--embedding-model",
            "cli-embed",
            "--rerank-provider",
            "jina",
            "--rerank-model",
            "cli-rerank",
            "-e",
            r#"(llm/embed "hello") (llm/rerank "query" ["first" "second"])"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.request().1["model"], "cli-embed");
    assert_eq!(server.request().1["model"], "cli-rerank");
}

#[test]
fn auto_configuration_uses_native_ollama_without_api_keys() {
    let server = Server::new();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .env_clear()
        .env("OLLAMA_HOST", server.url.trim_start_matches("http://"))
        .args(["-e", r#"(llm/embed "hello")"#])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (path, body) = server.request();
    assert_eq!(path, "/api/embed");
    assert_eq!(body["model"], "embeddinggemma");
}

#[test]
fn malformed_configuration_is_rejected() {
    for source in [
        r#"(llm/configure-embeddings :unknown {})"#,
        r#"(llm/configure-embeddings :nomic {})"#,
        r#"(llm/configure-embeddings :ollama {:rerank-model "unsupported"})"#,
        r#"(llm/configure-embeddings :openai {:model ""})"#,
        r#"(llm/configure-embeddings :openai {:dimensions 0})"#,
        r#"(llm/configure-embeddings :openai {:dimensions -1})"#,
        r#"(llm/configure-embeddings :openai {:timeout 0})"#,
        r#"(llm/configure-embeddings :ollama {:truncate "false"})"#,
        r#"(llm/configure-embeddings :jina {:top-k -1})"#,
    ] {
        assert!(interpreter().eval_str_compiled(source).is_err(), "{source}");
    }
}

#[test]
fn invalid_environment_selection_fails_before_running_the_program() {
    for variable in ["SEMA_EMBEDDING_PROVIDER", "SEMA_RERANK_PROVIDER"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
            .env_clear()
            .env(variable, "missing-provider")
            .args(["-e", "42"])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "invalid {variable} was silently ignored"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("not configured"));
    }
}

#[test]
#[ignore = "requires Vercel AI_GATEWAY_API_KEY"]
fn live_embedding_custom_vercel_gateway() {
    if std::env::var("AI_GATEWAY_API_KEY").map_or(true, |key| key.is_empty()) {
        return;
    }
    let result = interpreter().eval_str_compiled(r#"
        (llm/configure-embeddings :gateway {:api-key (env "AI_GATEWAY_API_KEY")
          :base-url "https://ai-gateway.vercel.sh/v1" :model "google/gemini-embedding-001" :dimensions 256 :timeout 30000})
        (list (map embedding/length (llm/embed ["cat" "dog"]))
              (embedding/length (llm/embed "cat" {:provider :gateway :model "google/gemini-embedding-001" :dimensions 128})))
    "#).unwrap();
    assert_eq!(result.to_string(), "((256 256) 128)");
}

#[test]
#[ignore = "requires Vercel AI_GATEWAY_API_KEY"]
fn live_rerank_vercel_gateway() {
    if std::env::var("AI_GATEWAY_API_KEY").map_or(true, |key| key.is_empty()) {
        return;
    }
    let result = interpreter().eval_str_compiled(r#"
        (llm/configure-embeddings :cohere {:api-key (env "AI_GATEWAY_API_KEY")
          :base-url "https://ai-gateway.vercel.sh/v2" :rerank-model "cohere/rerank-v3.5" :top-k 1 :timeout 30000})
        (llm/rerank "Where is the cat?" ["A cat sleeps on the sofa." "The weather is sunny."])
    "#).unwrap();
    let rows = result.as_list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_map_rc().unwrap()[&sema_core::Value::keyword("index")].as_int(),
        Some(0)
    );
}
