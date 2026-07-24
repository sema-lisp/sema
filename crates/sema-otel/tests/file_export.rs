//! M0 acceptance: `SEMA_OTEL_FILE` + a dropped span writes one JSON line, plus the C2
//! off-VM-thread contract (export runs on a dedicated writer thread; provider shutdown
//! flushes every span to disk).
//!
//! Each `#[test]` installs the process-global provider, so they must run in SEPARATE
//! processes — which `cargo nextest run` guarantees (one process per test). The unique
//! per-`process::id()` file paths keep them independent regardless.

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn file_exporter_writes_one_json_line_per_span() {
    use std::io::Read;

    let dir = std::env::temp_dir();
    let path = dir.join(format!("sema-otel-test-{}.jsonl", std::process::id()));
    let path_str = path.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&path);

    // SAFETY: single-threaded test setup before any otel init.
    unsafe {
        std::env::set_var("SEMA_OTEL_FILE", &path_str);
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }

    let guard = sema_otel::init_from_env();
    assert!(guard.is_some(), "SEMA_OTEL_FILE must install a provider");

    {
        let s = sema_otel::vm_span("unit-cell");
        s.set_str("sema.test", "yes");
        drop(s);

        let llm = sema_otel::llm_span("chat");
        llm.set_dispatch("gemini", "gemini-2.5-flash");
        llm.set_response(&sema_otel::ResponseFacts {
            input_tokens: 10,
            output_tokens: 5,
            response_model: "gemini-2.5-flash".into(),
            finish_reason: Some("stop".into()),
            cost_usd: Some(0.0001),
            ..Default::default()
        });
        drop(llm);
    }

    // Drop the guard → bounded flush + shutdown.
    drop(guard);

    let mut contents = String::new();
    std::fs::File::open(&path)
        .expect("jsonl file should exist")
        .read_to_string(&mut contents)
        .unwrap();
    let _ = std::fs::remove_file(&path);

    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "expected one JSON line per span, got:\n{contents}"
    );

    // Each line is valid JSON with the Sema schema.
    let vm: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(vm["name"], "unit-cell");
    assert_eq!(vm["kind"], "internal");
    assert_eq!(vm["attributes"]["sema.test"], "yes");

    // The LLM span: provider mapped (gemini → gcp.gemini), name renamed, usage present.
    let llm: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(llm["name"], "chat gemini-2.5-flash");
    assert_eq!(llm["kind"], "client");
    assert_eq!(llm["attributes"]["gen_ai.provider.name"], "gcp.gemini");
    assert_eq!(llm["attributes"]["gen_ai.operation.name"], "chat");
    assert_eq!(llm["attributes"]["gen_ai.usage.input_tokens"], 10);
    assert_eq!(llm["attributes"]["gen_ai.usage.output_tokens"], 5);
    assert_eq!(
        llm["attributes"]["gen_ai.response.finish_reasons"],
        serde_json::json!(["stop"])
    );
}

/// C2: span export must run on a dedicated writer thread, NOT the emitting (VM) thread.
/// The file writer records its own thread id when it starts; after we emit a span and
/// shut the provider down (which flushes through the writer), that recorded id must differ
/// from the thread that emitted the span.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn span_export_runs_on_a_dedicated_writer_thread() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("sema-otel-thread-{}.jsonl", std::process::id()));
    let path_str = path.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&path);

    // SAFETY: single-threaded test setup before any otel init.
    unsafe {
        std::env::set_var("SEMA_OTEL_FILE", &path_str);
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }

    let emitting_thread = std::thread::current().id();
    let guard = sema_otel::init_from_env();
    assert!(guard.is_some(), "SEMA_OTEL_FILE must install a provider");

    {
        let s = sema_otel::vm_span("thread-placement");
        s.set_str("sema.test", "yes");
        drop(s);
    }
    // Shutdown flushes through the writer, guaranteeing the writer thread has run (and
    // recorded its id) by the time this returns.
    drop(guard);
    let _ = std::fs::remove_file(&path);

    let writer_thread = sema_otel::last_writer_thread_id()
        .expect("the file writer thread must have started and recorded its id");
    assert_ne!(
        writer_thread, emitting_thread,
        "span export must run off the emitting/VM thread"
    );
}

/// C2: once the provider is shut down, every emitted span is on disk (the terminal flush
/// barrier drains the writer before shutdown returns). No span is lost to the off-thread
/// hand-off.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn all_spans_are_on_disk_after_provider_shutdown() {
    use std::io::Read;

    let dir = std::env::temp_dir();
    let path = dir.join(format!("sema-otel-flush-{}.jsonl", std::process::id()));
    let path_str = path.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&path);

    // SAFETY: single-threaded test setup before any otel init.
    unsafe {
        std::env::set_var("SEMA_OTEL_FILE", &path_str);
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }

    let guard = sema_otel::init_from_env();
    assert!(guard.is_some(), "SEMA_OTEL_FILE must install a provider");

    const N: usize = 25;
    for i in 0..N {
        let s = sema_otel::vm_span("flush-cell");
        s.set_str("sema.index", &i.to_string());
        drop(s);
    }

    // Provider shutdown parks on the terminal flush ack, so the file is complete on return.
    drop(guard);

    let mut contents = String::new();
    std::fs::File::open(&path)
        .expect("jsonl file should exist")
        .read_to_string(&mut contents)
        .unwrap();
    let _ = std::fs::remove_file(&path);

    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        N,
        "every emitted span must be flushed to disk after shutdown, got:\n{contents}"
    );
    // Each line is valid JSON with the expected span name.
    for line in &lines {
        let span: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(span["name"], "flush-cell");
    }
}
