//! Secret / PII detection and redaction builtins.
//!
//! Pattern-based scanners that find credentials (AWS keys, JWTs, Slack/GitHub
//! tokens, private-key blocks, generic `key = value` assignments, high-entropy
//! blobs) and personally-identifying data (emails, IPv4 addresses, phone
//! numbers), plus helpers to redact those spans out of text. Shannon entropy
//! gates the generic / high-entropy matchers so ordinary identifiers don't trip
//! a false positive.
//!
//! **Bounded / offloaded CPU (B8 R13 split).** `detect_secrets`/`detect_pii` run
//! many regex passes plus Shannon-entropy scans, so they are CPU-bound and can
//! run long on large text. During a runtime quantum (`in_runtime_quantum()`)
//! `secret/detect`, `secret/redact`, `pii/detect`, and `redact/spans` capture a
//! per-input byte cap BEFORE dispatch and offload the scan/redaction onto the I/O
//! pool through `quarantined_compute` (`io.rs`, the same mechanism
//! `archive.rs`/`diff.rs` use). The work runs over an owned `String` snapshot
//! (`Send`) on a worker thread and returns a `Send` result — the redacted text,
//! or `(text, findings)` where a `Finding` is just offsets plus a `&'static str`
//! kind — which is decoded back into a `Value` on the VM thread (where the
//! matched substrings are sliced out, since `Value`s can only be built there). No
//! `Value`/`Env` crosses the thread boundary. `hash/digest` is a plain O(input)
//! SHA-256, so it stays SYNCHRONOUS with only a pre-dispatch input-byte cap
//! (bounded input ⇒ bounded VM-thread CPU) — not a fake async wrap. A direct
//! native call outside the cooperative runtime keeps the uncapped synchronous
//! shape.

use std::collections::BTreeMap;

use sema_core::{check_arity, ArgsExt, SemaError, Value};
#[cfg(not(target_arch = "wasm32"))]
use sema_policy::content::INPUT_BYTE_CAP;
use sema_policy::content::{detect_pii, detect_secrets, redact as redact_findings, Finding};
use sha2::{Digest, Sha256};

use crate::register_fn;
#[cfg(not(target_arch = "wasm32"))]
use std::cell::Cell;
#[cfg(not(target_arch = "wasm32"))]
use {crate::register_runtime_fn, sema_core::runtime::NativeOutcome};

/// Per-input byte cap for the secret/PII ops under a runtime quantum. The
/// regex + entropy passes are heavier per byte than a plain line diff, so this
/// ceiling (16 MiB) is tighter than `diff`'s — still far above any realistic
/// credential-scan input.
#[cfg(not(target_arch = "wasm32"))]
const SECRET_INPUT_BYTE_CAP: u64 = INPUT_BYTE_CAP as u64;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// Optional per-call input-byte cap override (lowered, never raised above
    /// the hard ceiling). Read on the VM thread pre-dispatch; mirrors
    /// `git::GIT_MAX_OUTPUT_OVERRIDE`. `None` uses the module ceiling. The seam
    /// the regression suite drives to exercise the cap boundary without a
    /// multi-megabyte input string.
    static SECRET_INPUT_BYTE_CAP_OVERRIDE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The effective per-input byte cap for the current call: the module ceiling,
/// lowered by any per-call override (never raised above it).
#[cfg(not(target_arch = "wasm32"))]
fn effective_secret_input_byte_cap() -> u64 {
    crate::io::effective_cap(
        SECRET_INPUT_BYTE_CAP_OVERRIDE.with(Cell::get),
        SECRET_INPUT_BYTE_CAP,
    )
}

/// Lower the per-input byte cap (clamped to the hard ceiling) for subsequent
/// secret/PII calls on this thread, or clear the override with `None`. Test
/// seam, mirroring `set_git_max_output_bytes_override`.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_secret_input_byte_cap_override(bytes: Option<u64>) {
    SECRET_INPUT_BYTE_CAP_OVERRIDE.with(|cell| cell.set(bytes));
}

/// Reject `actual` bytes over `limit`. The check reads the argument's existing
/// `len()` — no snapshot is taken — so an over-cap input is rejected without any
/// excess allocation.
#[cfg(not(target_arch = "wasm32"))]
fn check_secret_limit(op: &str, actual: u64, limit: u64) -> Result<(), SemaError> {
    crate::io::check_quarantined_limit(
        op,
        "input bytes",
        actual,
        limit,
        "reduce or split the input text",
    )
}

/// Decode an offloaded redaction result (owned `String`) into a `Value` on the
/// VM thread. Non-capturing `fn` for `quarantined_compute`'s decoder slot.
#[cfg(not(target_arch = "wasm32"))]
fn secret_string_to_value(s: String) -> Value {
    Value::string(&s)
}

/// Decode an offloaded detect result — `(text, findings)`, both `Send` — into the
/// finding-map list on the VM thread (the matched substrings are sliced here).
/// Non-capturing `fn` for `quarantined_compute`'s decoder slot.
#[cfg(not(target_arch = "wasm32"))]
fn detect_pair_to_value(pair: (String, Vec<Finding>)) -> Value {
    let (text, findings) = pair;
    findings_to_list(&text, &findings)
}

/// Build the `{:type :match :start :end}` result map for a finding.
fn finding_to_map(text: &str, f: &Finding) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("type"), Value::string(f.label));
    m.insert(
        Value::keyword("match"),
        Value::string(&text[f.start..f.end]),
    );
    m.insert(Value::keyword("start"), Value::int(f.start as i64));
    m.insert(Value::keyword("end"), Value::int(f.end as i64));
    Value::map(m)
}

/// Turn a scan's `(text, findings)` pair into the `[{:type :match :start
/// :end} ...]` list `Value`. Shared by the sync and offloaded-async paths of
/// `secret/detect` and `pii/detect` so both build the identical result.
fn findings_to_list(text: &str, findings: &[Finding]) -> Value {
    let items: Vec<Value> = findings.iter().map(|f| finding_to_map(text, f)).collect();
    Value::list(items)
}

/// Parse a `redact/spans` span list into `Send` `(start, end, label)` edits,
/// skipping non-map / inverted / out-of-range / non-char-boundary spans. Reads
/// `Value`s, so it runs on the VM thread; the resulting edits are then applied by
/// [`apply_span_edits`] (which may run on an offload worker).
fn collect_span_edits(text: &str, spans: &[Value]) -> Vec<(usize, usize, Option<String>)> {
    let len = text.len();
    let mut edits: Vec<(usize, usize, Option<String>)> = Vec::new();
    for span in spans {
        let map = match span.as_map_ref() {
            Some(m) => m,
            None => continue, // skip non-map entries gracefully
        };
        let start = match map.get(&Value::keyword("start")).and_then(|v| v.as_int()) {
            Some(n) if n >= 0 => n as usize,
            _ => continue,
        };
        let end = match map.get(&Value::keyword("end")).and_then(|v| v.as_int()) {
            Some(n) if n >= 0 => n as usize,
            _ => continue,
        };
        // Clamp to the string and skip empty / inverted / non-char-boundary spans.
        let start = start.min(len);
        let end = end.min(len);
        if start >= end {
            continue;
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        let label = map
            .get(&Value::keyword("label"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        edits.push((start, end, label));
    }
    edits
}

/// Apply `(start, end, label)` edits to `text` right-to-left, dropping any span
/// that overlaps an already-accepted one (keeping the earliest-starting). Takes
/// `text` by value so the offloaded worker mutates the owned snapshot in place.
fn apply_span_edits(text: String, mut edits: Vec<(usize, usize, Option<String>)>) -> String {
    // Right-to-left replacement is only valid for NON-overlapping spans —
    // otherwise a later replace_range can index into a multibyte replacement
    // char («/») and panic. Drop spans that overlap an already-accepted one.
    edits.sort_by_key(|(start, _, _)| *start);
    let mut accepted: Vec<(usize, usize, Option<String>)> = Vec::new();
    let mut last_end = 0usize;
    for (start, end, label) in edits {
        if start >= last_end {
            last_end = end;
            accepted.push((start, end, label));
        }
    }

    // Apply from the rightmost span so earlier offsets remain valid.
    let mut out = text;
    for (start, end, label) in accepted.into_iter().rev() {
        let replacement = match &label {
            Some(l) => format!("\u{ab}redacted:{l}\u{bb}"),
            None => "\u{ab}redacted\u{bb}".to_string(),
        };
        out.replace_range(start..end, &replacement);
    }
    out
}

pub fn register(env: &sema_core::Env) {
    // secret/detect / secret/redact / pii/detect / redact/spans: the regex +
    // Shannon-entropy scan (and the redaction rewrite) is CPU-bound, so in a
    // runtime quantum each captures a per-input byte cap BEFORE dispatch and
    // offloads onto the I/O pool via `quarantined_compute`. The scan runs over an
    // owned `String` snapshot on a worker; the `Value` result is built back on
    // the VM thread (where the matched substrings are sliced). On wasm (no
    // cooperative runtime) they stay plainly synchronous.
    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn(env, "secret/detect", |args| {
        check_arity!(args, "secret/detect", 1);
        let s = args.str_at(0, "secret/detect")?;
        if sema_core::in_runtime_quantum() {
            check_secret_limit(
                "secret/detect",
                s.len() as u64,
                effective_secret_input_byte_cap(),
            )?;
            let snapshot = s.to_string();
            return crate::io::quarantined_compute(
                "secret/detect",
                detect_pair_to_value,
                move || {
                    let findings = detect_secrets(&snapshot);
                    Ok((snapshot, findings))
                },
            );
        }
        Ok(NativeOutcome::Return(findings_to_list(
            s,
            &detect_secrets(s),
        )))
    });
    #[cfg(target_arch = "wasm32")]
    register_fn(env, "secret/detect", |args| {
        check_arity!(args, "secret/detect", 1);
        let s = args.str_at(0, "secret/detect")?;
        Ok(findings_to_list(s, &detect_secrets(s)))
    });

    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn(env, "secret/redact", |args| {
        check_arity!(args, "secret/redact", 1);
        let s = args.str_at(0, "secret/redact")?;
        if sema_core::in_runtime_quantum() {
            check_secret_limit(
                "secret/redact",
                s.len() as u64,
                effective_secret_input_byte_cap(),
            )?;
            let snapshot = s.to_string();
            return crate::io::quarantined_compute(
                "secret/redact",
                secret_string_to_value,
                move || {
                    let findings = detect_secrets(&snapshot);
                    Ok(redact_findings(&snapshot, &findings))
                },
            );
        }
        Ok(NativeOutcome::Return(Value::string(&redact_findings(
            s,
            &detect_secrets(s),
        ))))
    });
    #[cfg(target_arch = "wasm32")]
    register_fn(env, "secret/redact", |args| {
        check_arity!(args, "secret/redact", 1);
        let s = args.str_at(0, "secret/redact")?;
        Ok(Value::string(&redact_findings(s, &detect_secrets(s))))
    });

    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn(env, "pii/detect", |args| {
        check_arity!(args, "pii/detect", 1);
        let s = args.str_at(0, "pii/detect")?;
        if sema_core::in_runtime_quantum() {
            check_secret_limit(
                "pii/detect",
                s.len() as u64,
                effective_secret_input_byte_cap(),
            )?;
            let snapshot = s.to_string();
            return crate::io::quarantined_compute("pii/detect", detect_pair_to_value, move || {
                let findings = detect_pii(&snapshot);
                Ok((snapshot, findings))
            });
        }
        Ok(NativeOutcome::Return(findings_to_list(s, &detect_pii(s))))
    });
    #[cfg(target_arch = "wasm32")]
    register_fn(env, "pii/detect", |args| {
        check_arity!(args, "pii/detect", 1);
        let s = args.str_at(0, "pii/detect")?;
        Ok(findings_to_list(s, &detect_pii(s)))
    });

    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn(env, "redact/spans", |args| {
        check_arity!(args, "redact/spans", 2);
        let text = args.str_at(0, "redact/spans")?;
        let spans = args.list_at(1, "redact/spans")?;
        let in_quantum = sema_core::in_runtime_quantum();
        if in_quantum {
            check_secret_limit(
                "redact/spans",
                text.len() as u64,
                effective_secret_input_byte_cap(),
            )?;
        }
        // Span parsing reads `Value`s, so it runs on the VM thread either way;
        // only the sort/dedup/rewrite over `text` is offloaded.
        let edits = collect_span_edits(text, spans);
        if in_quantum {
            let snapshot = text.to_string();
            return crate::io::quarantined_compute(
                "redact/spans",
                secret_string_to_value,
                move || Ok(apply_span_edits(snapshot, edits)),
            );
        }
        Ok(NativeOutcome::Return(Value::string(&apply_span_edits(
            text.to_string(),
            edits,
        ))))
    });
    #[cfg(target_arch = "wasm32")]
    register_fn(env, "redact/spans", |args| {
        check_arity!(args, "redact/spans", 2);
        let text = args.str_at(0, "redact/spans")?;
        let spans = args.list_at(1, "redact/spans")?;
        let edits = collect_span_edits(text, spans);
        Ok(Value::string(&apply_span_edits(text.to_string(), edits)))
    });

    // hash/digest is a plain O(input) SHA-256, so it stays SYNCHRONOUS; inside a
    // runtime quantum a pre-dispatch input-byte cap keeps its VM-thread CPU
    // bounded (a synchronous split, not a fake async wrap).
    register_fn(env, "hash/digest", |args| {
        check_arity!(args, "hash/digest", 1);
        let s = args.str_at(0, "hash/digest")?;
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_runtime_quantum() {
            check_secret_limit(
                "hash/digest",
                s.len() as u64,
                effective_secret_input_byte_cap(),
            )?;
        }
        let hash = Sha256::digest(s.as_bytes());
        let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
        Ok(Value::string(&hex))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{intern, Env, EvalContext, Value};

    /// Look up a registered builtin and invoke it with the given args.
    fn call(env: &Env, name: &str, args: &[Value]) -> Result<Value, SemaError> {
        let f = env
            .get(intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        let nf = f.as_native_fn_ref().expect("expected native fn");
        let ctx = EvalContext::default();
        (nf.func)(&ctx, args)
    }

    fn list_len(v: &Value) -> usize {
        v.as_list().expect("expected list").len()
    }

    /// First finding's `:type` value as an owned String.
    fn first_type(v: &Value) -> String {
        let items = v.as_list().expect("list");
        let m = items[0].as_map_ref().expect("map");
        m.get(&Value::keyword("type"))
            .and_then(|t| t.as_str())
            .expect("type string")
            .to_string()
    }

    #[test]
    fn detects_aws_access_key() {
        let env = Env::new();
        register(&env);
        let text = Value::string("AWS key AKIAIOSFODNN7EXAMPLE here");
        let r = call(&env, "secret/detect", &[text]).unwrap();
        assert_eq!(list_len(&r), 1);
        assert_eq!(first_type(&r), "aws-access-key");
    }

    #[test]
    fn detects_jwt_slack_github_and_private_key() {
        let env = Env::new();
        register(&env);

        let jwt =
            "token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcDEF123_-xyz";
        let r = call(&env, "secret/detect", &[Value::string(jwt)]).unwrap();
        assert!(r.as_list().unwrap().iter().any(|m| m
            .as_map_ref()
            .unwrap()
            .get(&Value::keyword("type"))
            .unwrap()
            .as_str()
            == Some("jwt")));

        // Placeholder tokens: structurally match the detectors but are all-zeros
        // so they aren't real secrets (and don't trip push protection).
        // Matches our loose detector (xox[baprs]-...) but deliberately NOT the
        // structured shape real Slack tokens (and scanners) use.
        let slack = "xoxb-EXAMPLE-NOT-A-REAL-TOKEN";
        let r = call(&env, "secret/detect", &[Value::string(slack)]).unwrap();
        assert_eq!(first_type(&r), "slack-token");

        let gh = "ghp_0000000000000000000000000000000000000000";
        let r = call(&env, "secret/detect", &[Value::string(gh)]).unwrap();
        assert_eq!(first_type(&r), "github-token");

        let pk = "-----BEGIN RSA PRIVATE KEY-----";
        let r = call(&env, "secret/detect", &[Value::string(pk)]).unwrap();
        assert_eq!(first_type(&r), "private-key");
    }

    #[test]
    fn generic_assignment_gated_by_entropy() {
        let env = Env::new();
        register(&env);
        // High-entropy value -> detected.
        let hit = "api_key = 'a8Fk3Lm9Zq2Wx7Bv1Nc4Pd6'";
        let r = call(&env, "secret/detect", &[Value::string(hit)]).unwrap();
        assert!(list_len(&r) >= 1);
    }

    #[test]
    fn redact_replaces_secret() {
        let env = Env::new();
        register(&env);
        let text = Value::string("key AKIAIOSFODNN7EXAMPLE done");
        let r = call(&env, "secret/redact", &[text]).unwrap();
        let s = r.as_str().unwrap();
        assert!(!s.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(s.contains("\u{ab}redacted:aws-access-key\u{bb}"));
    }

    #[test]
    fn pii_detect_email_ipv4_phone() {
        let env = Env::new();
        register(&env);

        let r = call(
            &env,
            "pii/detect",
            &[Value::string("contact me@example.com")],
        )
        .unwrap();
        assert_eq!(first_type(&r), "email");

        let r = call(&env, "pii/detect", &[Value::string("host 192.168.1.42 up")]).unwrap();
        assert_eq!(first_type(&r), "ipv4");

        let r = call(
            &env,
            "pii/detect",
            &[Value::string("call (415) 555-2671 now")],
        )
        .unwrap();
        assert_eq!(first_type(&r), "phone");
    }

    #[test]
    fn redact_spans_right_to_left() {
        let env = Env::new();
        register(&env);
        let text = "hello world foo";
        // Redact "world" (6..11) and "foo" (12..15).
        let mut s1 = BTreeMap::new();
        s1.insert(Value::keyword("start"), Value::int(6));
        s1.insert(Value::keyword("end"), Value::int(11));
        s1.insert(Value::keyword("label"), Value::string("name"));
        let mut s2 = BTreeMap::new();
        s2.insert(Value::keyword("start"), Value::int(12));
        s2.insert(Value::keyword("end"), Value::int(15));
        let spans = Value::list(vec![Value::map(s1), Value::map(s2)]);
        let r = call(&env, "redact/spans", &[Value::string(text), spans]).unwrap();
        assert_eq!(
            r.as_str().unwrap(),
            "hello \u{ab}redacted:name\u{bb} \u{ab}redacted\u{bb}"
        );
    }

    #[test]
    fn redact_spans_skips_invalid() {
        let env = Env::new();
        register(&env);
        let text = "abc";
        // Inverted span, out-of-range span, and a non-map entry — all skipped.
        let mut bad = BTreeMap::new();
        bad.insert(Value::keyword("start"), Value::int(5));
        bad.insert(Value::keyword("end"), Value::int(2));
        let spans = Value::list(vec![Value::map(bad), Value::int(99)]);
        let r = call(&env, "redact/spans", &[Value::string(text), spans]).unwrap();
        assert_eq!(r.as_str().unwrap(), "abc");
    }

    #[test]
    fn hash_digest_is_sha256_hex() {
        let env = Env::new();
        register(&env);
        let r = call(&env, "hash/digest", &[Value::string("")]).unwrap();
        // SHA-256 of empty string.
        assert_eq!(
            r.as_str().unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn secret_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_secret_limit("secret/detect", 8, 8).is_ok());
        let error = check_secret_limit("secret/detect", 9, 8)
            .expect_err("one byte over the captured limit must fail");
        assert!(error.to_string().contains('9'));
        assert!(error.to_string().contains('8'));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn secret_input_byte_cap_is_finite_and_clamps_overrides() {
        assert_eq!(effective_secret_input_byte_cap(), SECRET_INPUT_BYTE_CAP);
        set_secret_input_byte_cap_override(Some(16));
        assert_eq!(effective_secret_input_byte_cap(), 16);
        // An override above the hard ceiling is clamped down, never raised.
        set_secret_input_byte_cap_override(Some(u64::MAX));
        assert_eq!(effective_secret_input_byte_cap(), SECRET_INPUT_BYTE_CAP);
        set_secret_input_byte_cap_override(None);
        assert_eq!(effective_secret_input_byte_cap(), SECRET_INPUT_BYTE_CAP);
    }
}
