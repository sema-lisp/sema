//! Acceptance: `configure()` installs a provider programmatically (no env vars) and a
//! dropped span is written to the configured JSONL file. Own binary so the global-provider
//! guard runs cleanly. Mirrors `file_export.rs` but drives the install through `configure`.

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn configure_installs_provider_and_writes_spans() {
    use std::io::Read;

    let dir = std::env::temp_dir();
    let path = dir.join(format!("sema-otel-configure-{}.jsonl", std::process::id()));
    let path_str = path.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&path);

    // No SEMA_OTEL_FILE / OTLP endpoint in the env — configure entirely from code.
    // SAFETY: single-threaded test setup before any otel init.
    unsafe {
        std::env::remove_var("SEMA_OTEL_FILE");
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }

    let installed = sema_otel::configure(&sema_otel::OtelConfig {
        file: Some(path_str.clone()),
        service_name: Some("configure-test".into()),
        ..Default::default()
    });
    assert!(installed, "configure with :file must install a provider");

    // A second configure is a no-op (provider already installed).
    assert!(
        !sema_otel::configure(&sema_otel::OtelConfig {
            file: Some(path_str.clone()),
            ..Default::default()
        }),
        "second configure must be a no-op — one provider per process"
    );

    {
        let s = sema_otel::user_span("programmatic", sema_otel::SemaSpanKind::Internal, vec![]);
        s.set_str("sema.test", "configured");
        // The JSONL sink is a *simple* exporter: it writes + flushes on span-end (drop),
        // so the line is on disk immediately — no provider flush needed.
        drop(s);
    }

    let mut contents = String::new();
    std::fs::File::open(&path)
        .expect("jsonl file should exist")
        .read_to_string(&mut contents)
        .unwrap();
    let _ = std::fs::remove_file(&path);

    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "expected one span line, got:\n{contents}");

    let span: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(span["name"], "programmatic");
    assert_eq!(span["kind"], "internal");
    assert_eq!(span["attributes"]["sema.test"], "configured");
}
