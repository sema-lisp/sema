//! Reject corrupt retrieval responses before returning, recording, or using them.
use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

fn interpreter() -> Interpreter {
    let interp = Interpreter::new();
    reset_runtime_state();
    interp
}

#[test]
fn malformed_embeddings_are_rejected_in_root_and_spawned_calls() {
    for vectors in [
        vec![],
        vec![vec![]],
        vec![vec![f64::NAN]],
        vec![vec![f64::INFINITY]],
        vec![vec![1.0], vec![2.0]],
    ] {
        for spawned in [false, true] {
            let interp = interpreter();
            register_test_provider(Box::new(
                FakeProvider::builder("fake").embed(vectors.clone()).build(),
            ));
            let call = r#"(llm/embed "hello")"#;
            let source = if spawned {
                format!("(async/await (async/spawn (fn () {call})))")
            } else {
                call.to_string()
            };
            assert!(
                interp.eval_str_compiled(&source).is_err(),
                "accepted {vectors:?} (spawned={spawned})"
            );
        }
    }
}

#[test]
fn batch_embedding_count_and_dimensions_must_match() {
    for vectors in [vec![vec![1.0]], vec![vec![1.0], vec![2.0, 3.0]]] {
        let interp = interpreter();
        register_test_provider(Box::new(
            FakeProvider::builder("fake").embed(vectors.clone()).build(),
        ));
        assert!(
            interp
                .eval_str_compiled(r#"(llm/embed ["first" "second"])"#)
                .is_err(),
            "accepted {vectors:?}"
        );
    }
}

#[test]
fn empty_embedding_batch_returns_empty_without_dispatch() {
    let interp = interpreter();
    let fake = FakeProvider::builder("fake").build();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    assert_eq!(
        interp
            .eval_str_compiled("(llm/embed [])")
            .unwrap()
            .to_string(),
        "()"
    );
    assert!(recorder.embeds().is_empty());
}

#[test]
fn empty_rerank_batch_still_validates_options() {
    for source in [
        r#"(llm/rerank "query" [] 42)"#,
        r#"(llm/rerank "query" [] {:top-k -1})"#,
    ] {
        assert!(interpreter().eval_str_compiled(source).is_err(), "{source}");
    }
}

#[test]
fn malformed_rerank_results_are_rejected_in_root_and_spawned_calls() {
    for results in [
        vec![(2, 0.9)],
        vec![(0, f64::NAN)],
        vec![(0, 0.9), (0, 0.8)],
    ] {
        for spawned in [false, true] {
            let interp = interpreter();
            register_test_provider(Box::new(
                FakeProvider::builder("fake").rerank(&results).build(),
            ));
            let call = r#"(llm/rerank "query" ["first" "second"])"#;
            let source = if spawned {
                format!("(async/await (async/spawn (fn () {call})))")
            } else {
                call.to_string()
            };
            assert!(
                interp.eval_str_compiled(&source).is_err(),
                "accepted {results:?} (spawned={spawned})"
            );
        }
    }
}

#[test]
fn rerank_results_are_sorted_and_top_k_is_enforced() {
    let interp = interpreter();
    register_test_provider(Box::new(
        FakeProvider::builder("fake")
            .rerank(&[(0, 0.1), (1, 0.9)])
            .build(),
    ));
    let result = interp
        .eval_str_compiled(r#"(llm/rerank "query" ["first" "second"] {:top-k 1})"#)
        .unwrap();
    let rows = result.as_list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_map_rc().unwrap()[&sema_core::Value::keyword("document")].as_str(),
        Some("second")
    );
}

fn wire_result(
    response: serde_json::Value,
    operation: &str,
) -> Result<sema_core::Value, sema_core::SemaError> {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                if key.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap();
                }
            }
        }
        reader.read_exact(&mut vec![0; length]).unwrap();
        let body = response.to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let interp = interpreter();
    let provider = sema_llm::embeddings::OpenAiCompatEmbeddingProvider::new(
        "wire".into(),
        "test".into(),
        url,
        "test-model".into(),
    )
    .unwrap()
    .with_rerank(sema_llm::embeddings::RerankDialect::Jina);
    register_test_provider(Box::new(provider));
    let result = interp.eval_str_compiled(operation);
    server.join().unwrap();
    result
}

#[test]
fn embedding_response_indices_restore_input_order() {
    let result = wire_result(
        serde_json::json!({"data":[{"index":1,"embedding":[2.0]},{"index":0,"embedding":[1.0]}]}),
        r#"(map embedding/->list (llm/embed ["first" "second"]))"#,
    )
    .unwrap();
    assert_eq!(result.to_string(), "((1.0) (2.0))");
}

#[test]
fn corrupt_embedding_wire_values_are_not_silently_dropped() {
    use serde_json::json;
    for data in [
        json!([{"index":0,"embedding":[1.0,"bad"]}]),
        json!([{"index":0,"embedding":null}]),
        json!([{"index":1,"embedding":[1.0]}]),
        json!([{"index":-1,"embedding":[1.0]}]),
        json!([{"index":0,"embedding":[1.0]},{"index":0,"embedding":[2.0]}]),
    ] {
        assert!(
            wire_result(json!({"data":data}), r#"(llm/embed "hello")"#).is_err(),
            "accepted {data}"
        );
    }
}

#[test]
fn corrupt_rerank_wire_values_are_not_silently_dropped() {
    use serde_json::json;
    for results in [
        json!([{"index":0}]),
        json!([{"index":0,"relevance_score":"bad"}]),
        json!([{"index":-1,"relevance_score":0.5}]),
    ] {
        assert!(
            wire_result(
                json!({"results":results}),
                r#"(llm/rerank "query" ["hello"])"#
            )
            .is_err(),
            "accepted {results}"
        );
    }
}

#[test]
fn invalid_embeddings_are_never_recorded_and_corrupt_tapes_are_rejected() {
    use crate::common::sema_path;
    let path =
        std::env::temp_dir().join(format!("sema-invalid-embed-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let interp = interpreter();
    register_test_provider(Box::new(
        FakeProvider::builder("fake").embed(vec![]).build(),
    ));
    let source = format!(
        r#"(llm/with-cassette "{}" {{:mode :record}} (fn () (llm/embed "hello" {{:model "m"}})))"#,
        sema_path(&path)
    );
    assert!(interp.eval_str_compiled(&source).is_err());
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(!content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|row| row["kind"] == "embed"));

    register_test_provider(Box::new(
        FakeProvider::builder("fake").embed(vec![vec![1.0]]).build(),
    ));
    interp.eval_str_compiled(&source).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    let corrupt = content
        .lines()
        .map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
            if row["kind"] == "embed" {
                row["embeddings"] = serde_json::json!([]);
            }
            row.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{corrupt}\n")).unwrap();
    let result = interp.eval_str_compiled(&source.replace(":record", ":replay"));
    let _ = std::fs::remove_file(&path);
    assert!(
        result.is_err(),
        "corrupt tape yielded a valid-looking embedding"
    );
}
