//! Secret / PII detection and redaction builtins.
//!
//! Pattern-based scanners that find credentials (AWS keys, JWTs, Slack/GitHub
//! tokens, private-key blocks, generic `key = value` assignments, high-entropy
//! blobs) and personally-identifying data (emails, IPv4 addresses, phone
//! numbers), plus helpers to redact those spans out of text. Shannon entropy
//! gates the generic / high-entropy matchers so ordinary identifiers don't trip
//! a false positive.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use sema_core::{check_arity, SemaError, Value};
use sha2::{Digest, Sha256};

use crate::register_fn;

/// A single detected secret/PII finding.
struct Finding {
    kind: &'static str,
    start: usize,
    end: usize,
}

/// Shannon entropy in bits per character. Used to suppress low-entropy
/// (and therefore probably-not-secret) candidates for the generic and
/// high-entropy matchers.
fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts: BTreeMap<char, usize> = BTreeMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    let len = s.chars().count() as f64;
    let mut entropy = 0.0;
    for &count in counts.values() {
        let p = count as f64 / len;
        entropy -= p * p.log2();
    }
    entropy
}

/// Minimum bits/char of entropy for a generic or high-entropy candidate to be
/// treated as a real secret.
const ENTROPY_THRESHOLD: f64 = 3.5;

fn aws_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"AKIA[0-9A-Z]{16}").unwrap())
}

fn generic_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Capture group 2 is the secret value; the whole match (incl. the key and
    // operator) is what we report/redact.
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(api[_-]?key|secret|token|password)\s*[:=]\s*['"]?([A-Za-z0-9_\-]{16,})"#)
            .unwrap()
    })
}

fn private_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap())
}

fn jwt_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").unwrap())
}

fn slack_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"xox[baprs]-[A-Za-z0-9-]+").unwrap())
}

fn github_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"gh[pousr]_[A-Za-z0-9]{36,}").unwrap())
}

fn high_entropy_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Long hex or base64-ish runs (>= 32 chars). Entropy gate applied after.
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9+/=_\-]{32,}").unwrap())
}

fn email_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap())
}

fn ipv4_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Octet 0-255 with word boundaries so we don't grab digits mid-number.
    RE.get_or_init(|| {
        Regex::new(
            r"\b(?:(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])\.){3}(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])\b",
        )
        .unwrap()
    })
}

fn phone_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // US-style: optional +1, optional separators, 3-3-4 grouping.
    RE.get_or_init(|| {
        Regex::new(r"(?:\+?1[\s.\-]?)?\(?\d{3}\)?[\s.\-]?\d{3}[\s.\-]?\d{4}").unwrap()
    })
}

/// Push a finding only if it does not overlap an already-recorded one. Earlier
/// matchers (more specific patterns) win over the broad high-entropy matcher.
fn push_if_free(findings: &mut Vec<Finding>, kind: &'static str, start: usize, end: usize) {
    let overlaps = findings.iter().any(|f| start < f.end && f.start < end);
    if !overlaps {
        findings.push(Finding { kind, start, end });
    }
}

/// Run all secret matchers over `text`, returning non-overlapping findings
/// sorted by start offset. Order of matchers matters: specific patterns are
/// recorded first so the generic / high-entropy matchers can't shadow them.
fn detect_secrets(text: &str) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();

    for m in aws_re().find_iter(text) {
        push_if_free(&mut findings, "aws-access-key", m.start(), m.end());
    }
    for m in private_key_re().find_iter(text) {
        push_if_free(&mut findings, "private-key", m.start(), m.end());
    }
    for m in jwt_re().find_iter(text) {
        push_if_free(&mut findings, "jwt", m.start(), m.end());
    }
    for m in slack_re().find_iter(text) {
        push_if_free(&mut findings, "slack-token", m.start(), m.end());
    }
    for m in github_re().find_iter(text) {
        push_if_free(&mut findings, "github-token", m.start(), m.end());
    }
    // Generic `key = value` assignments — gate the captured value on entropy.
    for caps in generic_re().captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let value = caps.get(2).unwrap();
        if shannon_entropy(value.as_str()) >= ENTROPY_THRESHOLD {
            push_if_free(&mut findings, "api-key", whole.start(), whole.end());
        }
    }
    // Bare high-entropy blobs (hex/base64) that none of the above caught.
    for m in high_entropy_re().find_iter(text) {
        if shannon_entropy(m.as_str()) >= ENTROPY_THRESHOLD {
            push_if_free(&mut findings, "high-entropy", m.start(), m.end());
        }
    }

    findings.sort_by_key(|f| f.start);
    findings
}

/// Run all PII matchers over `text`, returning non-overlapping findings sorted
/// by start offset.
fn detect_pii(text: &str) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();

    for m in email_re().find_iter(text) {
        push_if_free(&mut findings, "email", m.start(), m.end());
    }
    for m in ipv4_re().find_iter(text) {
        push_if_free(&mut findings, "ipv4", m.start(), m.end());
    }
    for m in phone_re().find_iter(text) {
        push_if_free(&mut findings, "phone", m.start(), m.end());
    }

    findings.sort_by_key(|f| f.start);
    findings
}

/// Build the `{:type :match :start :end}` result map for a finding.
fn finding_to_map(text: &str, f: &Finding) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("type"), Value::string(f.kind));
    m.insert(
        Value::keyword("match"),
        Value::string(&text[f.start..f.end]),
    );
    m.insert(Value::keyword("start"), Value::int(f.start as i64));
    m.insert(Value::keyword("end"), Value::int(f.end as i64));
    Value::map(m)
}

/// Replace each finding's span with `«redacted:<type>»`, working right-to-left
/// so byte offsets of not-yet-applied edits stay valid.
fn redact_findings(text: &str, findings: &[Finding]) -> String {
    let mut out = text.to_string();
    for f in findings.iter().rev() {
        let replacement = format!("\u{ab}redacted:{}\u{bb}", f.kind);
        out.replace_range(f.start..f.end, &replacement);
    }
    out
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "secret/detect", |args| {
        check_arity!(args, "secret/detect", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let findings = detect_secrets(s);
        let items: Vec<Value> = findings.iter().map(|f| finding_to_map(s, f)).collect();
        Ok(Value::list(items))
    });

    register_fn(env, "secret/redact", |args| {
        check_arity!(args, "secret/redact", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let findings = detect_secrets(s);
        Ok(Value::string(&redact_findings(s, &findings)))
    });

    register_fn(env, "pii/detect", |args| {
        check_arity!(args, "pii/detect", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let findings = detect_pii(s);
        let items: Vec<Value> = findings.iter().map(|f| finding_to_map(s, f)).collect();
        Ok(Value::list(items))
    });

    register_fn(env, "redact/spans", |args| {
        check_arity!(args, "redact/spans", 2);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let spans = args[1]
            .as_list()
            .ok_or_else(|| SemaError::type_error("list", args[1].type_name()))?;

        // Collect (start, end, label) for valid spans, then apply right-to-left.
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

        // Right-to-left replacement is only valid for NON-overlapping spans —
        // otherwise a later replace_range can index into a multibyte
        // replacement char («/») and panic. Drop spans that overlap an
        // already-accepted one (keeping the earliest-starting).
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
        let mut out = text.to_string();
        for (start, end, label) in accepted.into_iter().rev() {
            let replacement = match &label {
                Some(l) => format!("\u{ab}redacted:{l}\u{bb}"),
                None => "\u{ab}redacted\u{bb}".to_string(),
            };
            out.replace_range(start..end, &replacement);
        }
        Ok(Value::string(&out))
    });

    register_fn(env, "hash/digest", |args| {
        check_arity!(args, "hash/digest", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
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

    #[test]
    fn entropy_low_for_repetitive() {
        assert!(shannon_entropy("aaaaaaaa") < 1.0);
        assert!(shannon_entropy("a8Fk3Lm9Zq2Wx7Bv1Nc4Pd6") >= ENTROPY_THRESHOLD);
    }
}
