//! `GET /api/run/:id/auth` — MCP auth status for the dashboard.
//!
//! Derives its baseline answer ONLY from the run directory's own frozen
//! artifacts: the redacted `:mcp` manifest recorded in `metadata.json`
//! (aliases, url/command, whether `:auth` was declared, `:tools`, `:persist`)
//! and the LATEST relevant `auth.*` event per alias across `events.jsonl` +
//! every `events.resume-<n>.jsonl` segment, in segment order. This module
//! itself never touches the token store — that needs key material and belongs
//! to the write endpoints (`super::connect`) — so it can never leak a token,
//! header, or other credential material by itself.
//!
//! On top of that journal-derived baseline, `auth_status_json` merges in-memory
//! [`FlowState`] overrides recorded by a same-process `connect`/`forget` call
//! (`super::connect::ServerState::flows`): a pending/just-finished dashboard
//! login for an alias overrides that alias's row entirely — `Connecting` ->
//! `"connecting"`, `Authorized` -> `"authorized"` (+ its `expires_at`),
//! `Failed` -> `"failed"` (+ `reason`) — so the panel sees the flow's outcome
//! immediately, without waiting for a NEW run to write a fresh `auth.*` event.
//! An alias with no override falls back to the journal derivation, unchanged.
//!
//! Degrades to `[]` on anything missing or malformed, same discipline as
//! `workflow_view::index_runs_json` — this endpoint never 500s the viewer.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::connect::FlowState;

/// The latest `auth.*` outcome seen for one alias, folded across journal segments.
#[derive(Debug, Clone, PartialEq)]
enum AuthEvent {
    Required,
    Granted { expires_at: Option<u64> },
    Failed,
}

/// `application/json` body for `GET /api/run/<id>/auth`: one row per alias declared
/// in the run's `:mcp` manifest, `[]` when there is none (or the run's artifacts
/// can't be read). Callers must pass an `id` that already passed
/// [`super::is_safe_segment`] — this function only ever joins `run_dir.join(id)`,
/// so it can't escape `run_dir` even so, but the route-level check is what keeps a
/// traversal-shaped `id` out of the filesystem calls entirely.
///
/// `overrides` is this run's in-memory flow-state snapshot (alias -> latest
/// `connect`/`forget` outcome), from `super::connect::ServerState::flow_snapshot`
/// — empty for every request until a dashboard `connect` has run at least once.
///
/// `pub(crate)`, not `pub`: [`FlowState`] itself is `pub(crate)` (server-side
/// flow state never needs to leave this crate), so this stays in step —
/// `super::route` (the only caller) is in the same crate.
pub(crate) fn auth_status_json(
    run_dir: &Path,
    id: &str,
    overrides: &HashMap<String, FlowState>,
) -> Vec<u8> {
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    status_json_merged(run_dir, id, now_unix, overrides)
}

/// [`auth_status_json`] with no flow overrides — the pure journal-derived
/// answer. Kept separate (rather than threading an empty map through every
/// call site) so the read-only-status tests below stay untouched by the
/// write-path's addition. Test-only: every non-test caller already has (or
/// can trivially get) a real overrides map via [`auth_status_json`].
#[cfg(test)]
fn status_json_at(run_dir: &Path, id: &str, now_unix: u64) -> Vec<u8> {
    status_json_merged(run_dir, id, now_unix, &HashMap::new())
}

fn status_json_merged(
    run_dir: &Path,
    id: &str,
    now_unix: u64,
    overrides: &HashMap<String, FlowState>,
) -> Vec<u8> {
    let rows = build_rows(run_dir, id, now_unix, overrides);
    serde_json::to_vec(&serde_json::Value::Array(rows)).unwrap_or_else(|_| b"[]".to_vec())
}

fn build_rows(
    run_dir: &Path,
    id: &str,
    now_unix: u64,
    overrides: &HashMap<String, FlowState>,
) -> Vec<serde_json::Value> {
    let dir = run_dir.join(id);
    let Some(mcp) = read_mcp_manifest(&dir) else {
        return Vec::new();
    };
    let latest = latest_auth_events(&dir);
    mcp.iter()
        .map(|(alias, spec)| {
            row_for(
                alias,
                spec,
                latest.get(alias.as_str()),
                now_unix,
                overrides.get(alias.as_str()),
            )
        })
        .collect()
}

/// `meta.mcp` from `metadata.json`, already secret-redacted by the runtime
/// (`crate::workflow`'s `redact_meta_secrets`, upstream of this module). `None`
/// when the file is missing/corrupt, `meta` isn't an object, or there is no `:mcp`
/// key (a run that declared no MCP servers) — every one of those folds to the same
/// empty `[]` response.
fn read_mcp_manifest(dir: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let text = std::fs::read_to_string(dir.join("metadata.json")).ok()?;
    let meta: serde_json::Value = serde_json::from_str(&text).ok()?;
    meta.get("meta")?.get("mcp")?.as_object().cloned()
}

/// `events.jsonl` then `events.resume-1.jsonl`, `events.resume-2.jsonl`, … in
/// order, stopping at the first missing segment — mirrors the segment walk in
/// `workflow_view::ingest::sync_run`.
fn journal_segments(dir: &Path) -> Vec<PathBuf> {
    let mut segs = vec![dir.join("events.jsonl")];
    let mut n: u32 = 1;
    loop {
        let seg = dir.join(format!("events.resume-{n}.jsonl"));
        if !seg.exists() {
            break;
        }
        segs.push(seg);
        n += 1;
    }
    segs
}

/// The LATEST `auth.*` event per alias (`server` field), folded across every
/// journal segment in order — a later segment's event overrides an earlier one for
/// the same alias, matching how a resume re-gates
/// (`docs/plans/2026-06-24-workflow-mcp-auth.md` §7: conservative resume). A
/// malformed/truncated line is skipped, never fatal.
fn latest_auth_events(dir: &Path) -> BTreeMap<String, AuthEvent> {
    let mut latest = BTreeMap::new();
    for path in journal_segments(dir) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(event) = v.get("event").and_then(|x| x.as_str()) else {
                continue;
            };
            let Some(server) = v.get("server").and_then(|x| x.as_str()) else {
                continue;
            };
            let outcome = match event {
                "auth.required" => AuthEvent::Required,
                "auth.granted" => AuthEvent::Granted {
                    expires_at: v.get("expires_at").and_then(|x| x.as_u64()),
                },
                "auth.failed" => AuthEvent::Failed,
                _ => continue,
            };
            latest.insert(server.to_string(), outcome);
        }
    }
    latest
}

fn str_field(spec: &serde_json::Value, key: &str) -> Option<String> {
    spec.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn str_list_field(spec: &serde_json::Value, key: &str) -> Vec<String> {
    spec.get(key)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Build one response row from a manifest spec + its folded journal status,
/// then let a same-process flow override (if any) win entirely — see the
/// module doc. Reads ONLY `url`/`command`/`auth`/`tools`/`persist` off `spec`
/// — `headers`/`env` (already `<redacted>`, but redaction is defense in depth,
/// not a license to forward them) are never touched, so no header/token
/// material can reach this endpoint's response even by accident.
fn row_for(
    alias: &str,
    spec: &serde_json::Value,
    latest: Option<&AuthEvent>,
    now_unix: u64,
    flow_override: Option<&FlowState>,
) -> serde_json::Value {
    let auth = spec.get("auth").filter(|a| a.is_object());
    let needs_auth = auth.is_some();
    let scopes = auth
        .map(|a| str_list_field(a, "scopes"))
        .unwrap_or_default();
    let tools = str_list_field(spec, "tools");
    let persist = str_field(spec, "persist").unwrap_or_else(|| "workflow".to_string());
    let url = str_field(spec, "url");
    let command = str_field(spec, "command");

    let journal_status = || -> (&'static str, Option<u64>) {
        match latest {
            Some(AuthEvent::Granted { expires_at }) => {
                if expires_at.is_some_and(|exp| exp < now_unix) {
                    ("expired", None)
                } else {
                    ("authorized", *expires_at)
                }
            }
            Some(AuthEvent::Required) => ("needs-consent", None),
            Some(AuthEvent::Failed) => ("failed", None),
            // No auth.* event at all: a declared-with-:auth server that never ran the
            // resolution step reads as "needs-consent" (never a false "authorized");
            // a server with no :auth at all needs no flow, so "open" — never
            // "authorized" for a server that was simply never gated.
            None if needs_auth => ("needs-consent", None),
            None => ("open", None),
        }
    };

    // A same-process flow state, when present, overrides the journal-derived
    // status entirely — it is strictly newer information (this process's own
    // in-flight or just-finished `connect`), and a `Failed` flow carries a
    // `reason` the journal alone never would.
    let (status, expires_at, reason) = match flow_override {
        Some(FlowState::Connecting) => ("connecting", None, None),
        Some(FlowState::Authorized { expires_at }) => ("authorized", *expires_at, None),
        Some(FlowState::Failed { reason }) => ("failed", None, Some(reason.clone())),
        None => {
            let (status, expires_at) = journal_status();
            (status, expires_at, None)
        }
    };

    let mut row = serde_json::Map::new();
    row.insert("alias".into(), serde_json::Value::String(alias.to_string()));
    row.insert("needs_auth".into(), serde_json::Value::Bool(needs_auth));
    row.insert(
        "status".into(),
        serde_json::Value::String(status.to_string()),
    );
    if !scopes.is_empty() {
        row.insert("scopes".into(), serde_json::json!(scopes));
    }
    row.insert("tools".into(), serde_json::json!(tools));
    row.insert("persist".into(), serde_json::Value::String(persist));
    if let Some(u) = url {
        row.insert("url".into(), serde_json::Value::String(u));
    }
    if let Some(c) = command {
        row.insert("command".into(), serde_json::Value::String(c));
    }
    if let Some(exp) = expires_at {
        row.insert("expires_at".into(), serde_json::json!(exp));
    }
    if let Some(r) = reason {
        row.insert("reason".into(), serde_json::Value::String(r));
    }
    serde_json::Value::Object(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "sema-wf-auth-status-{}-{}-{tag}",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn no_mcp_key_yields_empty_array() {
        let root = temp_dir("no-mcp");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(&run, "metadata.json", r#"{"meta":{"budget":{"usd":1.0}}}"#);

        assert_eq!(status_json_at(&root, "r1", 0), b"[]");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_metadata_yields_empty_array() {
        let root = temp_dir("missing-meta");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        // No metadata.json at all.
        assert_eq!(status_json_at(&root, "r1", 0), b"[]");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_metadata_yields_empty_array() {
        let root = temp_dir("corrupt-meta");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(&run, "metadata.json", "{not json");
        assert_eq!(status_json_at(&root, "r1", 0), b"[]");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_run_dir_yields_empty_array() {
        let root = temp_dir("no-run-dir");
        assert_eq!(status_json_at(&root, "does-not-exist", 0), b"[]");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn exact_json_across_every_status() {
        let root = temp_dir("exact-json");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(
            &run,
            "metadata.json",
            r#"{"meta":{"mcp":{
                "asana": {"url":"https://mcp.asana.com/mcp",
                          "headers":{"Authorization":"<redacted>"},
                          "auth":{"scopes":["default"]},
                          "tools":["create_task"],
                          "persist":"workflow"},
                "fs": {"command":"npx"}
            }}}"#,
        );
        write(
            &run,
            "events.jsonl",
            concat!(
                r#"{"event":"auth.required","seq":0,"ts":"0","server":"asana","scopes":["default"],"tools":["create_task"],"persist":"workflow"}"#,
                "\n"
            ),
        );

        let json = status_json_at(&root, "r1", 1_000);
        let text = String::from_utf8(json).unwrap();
        assert_eq!(
            text,
            r#"[{"alias":"asana","needs_auth":true,"persist":"workflow","scopes":["default"],"status":"needs-consent","tools":["create_task"],"url":"https://mcp.asana.com/mcp"},{"alias":"fs","command":"npx","needs_auth":false,"persist":"workflow","status":"open","tools":[]}]"#
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn granted_with_future_expiry_is_authorized_and_carries_expires_at() {
        let root = temp_dir("granted-future");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(
            &run,
            "metadata.json",
            r#"{"meta":{"mcp":{"asana":{"url":"https://mcp.asana.com/mcp","auth":{"scopes":["default"]},"persist":"run"}}}}"#,
        );
        write(
            &run,
            "events.jsonl",
            concat!(
                r#"{"event":"auth.required","seq":0,"ts":"0","server":"asana","persist":"run"}"#,
                "\n",
                r#"{"event":"auth.granted","seq":1,"ts":"0","server":"asana","expires_at":2000,"source":"consented"}"#,
                "\n",
            ),
        );

        let text = String::from_utf8(status_json_at(&root, "r1", 1_000)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["status"], "authorized");
        assert_eq!(v[0]["expires_at"], 2000);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expiry_boundary_equal_to_now_is_still_authorized() {
        let root = temp_dir("expiry-eq");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(
            &run,
            "metadata.json",
            r#"{"meta":{"mcp":{"asana":{"url":"https://x","auth":{}}}}}"#,
        );
        write(
            &run,
            "events.jsonl",
            r#"{"event":"auth.granted","seq":0,"ts":"0","server":"asana","expires_at":1000,"source":"cached"}"#,
        );

        let now_equal = String::from_utf8(status_json_at(&root, "r1", 1000)).unwrap();
        assert!(
            now_equal.contains(r#""status":"authorized""#),
            "{now_equal}"
        );

        let now_past = String::from_utf8(status_json_at(&root, "r1", 1001)).unwrap();
        assert!(now_past.contains(r#""status":"expired""#), "{now_past}");
        // An expired row never carries a stale expires_at into the response.
        assert!(!now_past.contains("expires_at"), "{now_past}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_latest_event_wins_over_an_earlier_grant() {
        let root = temp_dir("failed-latest");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(
            &run,
            "metadata.json",
            r#"{"meta":{"mcp":{"asana":{"url":"https://x","auth":{}}}}}"#,
        );
        write(
            &run,
            "events.jsonl",
            concat!(
                r#"{"event":"auth.granted","seq":0,"ts":"0","server":"asana","source":"cached"}"#,
                "\n",
                r#"{"event":"auth.failed","seq":1,"ts":"0","server":"asana","reason":"consent_denied"}"#,
                "\n",
            ),
        );
        let text = String::from_utf8(status_json_at(&root, "r1", 0)).unwrap();
        assert!(text.contains(r#""status":"failed""#), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resume_segment_overrides_primary_segment_status() {
        let root = temp_dir("resume-override");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(
            &run,
            "metadata.json",
            r#"{"meta":{"mcp":{"asana":{"url":"https://x","auth":{"scopes":["default"]},"persist":"run"}}}}"#,
        );
        // Primary segment: gate never cleared.
        write(
            &run,
            "events.jsonl",
            r#"{"event":"auth.required","seq":0,"ts":"0","server":"asana","persist":"run"}"#,
        );
        // Resume segment: the resumed run's auth-resolution step finds a granted session.
        write(
            &run,
            "events.resume-1.jsonl",
            r#"{"event":"auth.granted","seq":0,"ts":"0","server":"asana","source":"refreshed"}"#,
        );

        let text = String::from_utf8(status_json_at(&root, "r1", 0)).unwrap();
        assert!(
            text.contains(r#""status":"authorized""#),
            "resume segment must override the primary segment's needs-consent: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn malformed_journal_line_is_skipped_not_fatal() {
        let root = temp_dir("malformed-line");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(
            &run,
            "metadata.json",
            r#"{"meta":{"mcp":{"asana":{"url":"https://x","auth":{}}}}}"#,
        );
        write(
            &run,
            "events.jsonl",
            concat!(
                "not even json\n",
                r#"{"event":"auth.granted","seq":0,"ts":"0","server":"asana","source":"cached"}"#,
                "\n",
            ),
        );
        let text = String::from_utf8(status_json_at(&root, "r1", 0)).unwrap();
        assert!(text.contains(r#""status":"authorized""#), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn headers_and_env_never_reach_the_response_even_if_unredacted() {
        // Defense in depth: even if a caller somehow wrote unredacted secrets into
        // metadata.json (redaction is the runtime's job, not this endpoint's), this
        // endpoint must never forward `headers`/`env` — it only ever reads
        // url/command/auth/tools/persist off the spec.
        let root = temp_dir("no-secrets");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        write(
            &run,
            "metadata.json",
            r#"{"meta":{"mcp":{"asana":{
                "url":"https://mcp.asana.com/mcp",
                "headers":{"Authorization":"Bearer not-actually-redacted-in-this-test"},
                "auth":{"scopes":["default"]},
                "persist":"workflow"
            }}}}"#,
        );
        let text = String::from_utf8(status_json_at(&root, "r1", 0)).unwrap();
        assert!(!text.contains("Bearer"), "{text}");
        assert!(!text.contains("headers"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Task 10: in-memory flow-state overrides (`super::connect`) ────────────

    fn manifest_one_http_alias(run: &Path) {
        write(
            run,
            "metadata.json",
            r#"{"workflow":"w","meta":{"mcp":{"asana":{"url":"https://mcp.asana.com/mcp","auth":{"scopes":["default"]},"persist":"workflow"}}}}"#,
        );
        write(
            run,
            "events.jsonl",
            r#"{"event":"auth.required","seq":0,"ts":"0","server":"asana","scopes":["default"],"persist":"workflow"}"#,
        );
    }

    #[test]
    fn connecting_override_wins_over_needs_consent_journal_status() {
        let root = temp_dir("override-connecting");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        manifest_one_http_alias(&run);

        let mut overrides = HashMap::new();
        overrides.insert("asana".to_string(), FlowState::Connecting);

        let text = String::from_utf8(status_json_merged(&root, "r1", 0, &overrides)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["status"], "connecting");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn authorized_override_wins_and_carries_expires_at() {
        let root = temp_dir("override-authorized");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        manifest_one_http_alias(&run);

        let mut overrides = HashMap::new();
        overrides.insert(
            "asana".to_string(),
            FlowState::Authorized {
                expires_at: Some(9999),
            },
        );

        let text = String::from_utf8(status_json_merged(&root, "r1", 0, &overrides)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["status"], "authorized");
        assert_eq!(v[0]["expires_at"], 9999);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_override_wins_and_carries_reason_never_a_secret() {
        let root = temp_dir("override-failed");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        manifest_one_http_alias(&run);

        let mut overrides = HashMap::new();
        overrides.insert(
            "asana".to_string(),
            FlowState::Failed {
                reason: "consent declined".to_string(),
            },
        );

        let text = String::from_utf8(status_json_merged(&root, "r1", 0, &overrides)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["status"], "failed");
        assert_eq!(v[0]["reason"], "consent declined");
        assert!(!text.contains("Bearer"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_override_falls_back_to_journal_status_unchanged() {
        let root = temp_dir("override-absent");
        let run = root.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        manifest_one_http_alias(&run);

        // An override present for a DIFFERENT alias must not affect this one.
        let mut overrides = HashMap::new();
        overrides.insert("other-alias".to_string(), FlowState::Connecting);

        let merged = String::from_utf8(status_json_merged(&root, "r1", 0, &overrides)).unwrap();
        let plain = String::from_utf8(status_json_at(&root, "r1", 0)).unwrap();
        assert_eq!(merged, plain);
        assert!(merged.contains(r#""status":"needs-consent""#), "{merged}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
