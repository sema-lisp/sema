//! LLM cassettes — record/replay for deterministic, keyless LLM testing & demos.
//!
//! A cassette intercepts completions at a seam *below* the OpenTelemetry span, the
//! response cache, and usage/cost accounting (all of which live in `do_complete`),
//! and *above* the real provider. That layering is deliberate:
//!
//! - **OTel:** a replayed call still flows through `do_complete`, so it emits the
//!   same `chat` span — populated from the *recorded* model/usage. Replayed traces
//!   look like real ones.
//! - **Usage/cost:** a replay returns the recorded [`Usage`] (including prompt-cache
//!   tokens), so `track_usage`, budgets, and cost math all exercise deterministically.
//!   This is distinct from a *cache hit*, which reports zero usage (no call was
//!   "made"); a replay is a stand-in for a real call, so it carries real numbers.
//! - **Response cache:** the cassette sits below the cache. To avoid a cache hit
//!   short-circuiting before the tape is consulted, `with-cassette` disables the
//!   response cache for its dynamic extent.
//!
//! The tape stores only the *response* keyed by a hash of the request — never the
//! request body, API keys, or headers — so redaction is guaranteed by construction.

use crate::types::{ChatResponse, ToolCall, Usage};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How a cassette behaves on each call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CassetteMode {
    /// Always hit the real provider AND append each interaction to the tape.
    Record,
    /// Never hit the provider; serve from the tape. A miss is a hard error.
    Replay,
    /// Replay if the tape has the call, else record it (the dev/test default).
    Auto,
}

impl CassetteMode {
    /// Parse `record` / `replay` / `auto` (case-insensitive). Unknown => `Auto`.
    pub fn parse(s: &str) -> CassetteMode {
        match s.trim().to_ascii_lowercase().as_str() {
            "record" => CassetteMode::Record,
            "replay" => CassetteMode::Replay,
            _ => CassetteMode::Auto,
        }
    }
}

/// One recorded interaction. Serializable on its own (the core LLM types don't
/// derive serde), holding exactly what's needed to rebuild a [`ChatResponse`].
/// No request content is stored — only its hash `key` — so no prompt text, API
/// key, or header can ever land on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapeEntry {
    /// Tape format version (migration hook).
    pub v: u32,
    /// Interaction kind — `"complete"` today; open for `"stream"`/`"embed"`/`"mcp-call"`.
    pub kind: String,
    /// Request hash (the matching key).
    pub key: String,
    pub content: String,
    #[serde(default = "default_role")]
    pub role: String,
    pub model: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    /// For `kind:"stream"` — the recorded text chunks in order. Replay feeds these
    /// to the caller's `on_chunk` to reproduce the same boundaries.
    #[serde(default)]
    pub chunks: Vec<String>,
    /// For `kind:"embed"` — the recorded embedding vectors (one per input text).
    #[serde(default)]
    pub embeddings: Vec<Vec<f64>>,
    /// For `kind:"mcp-call"` — the recorded `tools/call` result JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_result: Option<serde_json::Value>,
}

fn default_role() -> String {
    "assistant".to_string()
}

impl TapeEntry {
    /// Build a tape entry from a live response under `key`.
    pub fn from_response(key: &str, resp: &ChatResponse) -> TapeEntry {
        TapeEntry {
            v: 1,
            kind: "complete".to_string(),
            key: key.to_string(),
            content: resp.content.clone(),
            role: resp.role.clone(),
            model: resp.model.clone(),
            tool_calls: resp.tool_calls.clone(),
            stop_reason: resp.stop_reason.clone(),
            prompt_tokens: resp.usage.prompt_tokens,
            completion_tokens: resp.usage.completion_tokens,
            cache_read_input_tokens: resp.usage.cache_read_input_tokens,
            cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
            chunks: Vec::new(),
            embeddings: Vec::new(),
            mcp_result: None,
        }
    }

    /// Tape entry for an MCP `tools/call`: the recorded result JSON, keyed by a
    /// hash of the server identity + tool + arguments.
    pub fn from_mcp_call(key: &str, result: &serde_json::Value) -> TapeEntry {
        TapeEntry {
            v: 1,
            kind: "mcp-call".to_string(),
            key: key.to_string(),
            content: String::new(),
            role: default_role(),
            model: String::new(),
            tool_calls: Vec::new(),
            stop_reason: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            chunks: Vec::new(),
            embeddings: Vec::new(),
            mcp_result: Some(result.clone()),
        }
    }

    /// Tape entry for a streamed completion: the chunk sequence plus the final response.
    pub fn from_stream(key: &str, chunks: &[String], resp: &ChatResponse) -> TapeEntry {
        let mut entry = TapeEntry::from_response(key, resp);
        entry.kind = "stream".to_string();
        entry.chunks = chunks.to_vec();
        entry
    }

    /// Tape entry for an embeddings call: the vectors plus the model and input tokens.
    pub fn from_embed(
        key: &str,
        model: &str,
        embeddings: &[Vec<f64>],
        prompt_tokens: u32,
    ) -> TapeEntry {
        TapeEntry {
            v: 1,
            kind: "embed".to_string(),
            key: key.to_string(),
            content: String::new(),
            role: default_role(),
            model: model.to_string(),
            tool_calls: Vec::new(),
            stop_reason: None,
            prompt_tokens,
            completion_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            chunks: Vec::new(),
            embeddings: embeddings.to_vec(),
            mcp_result: None,
        }
    }

    /// Reconstruct a [`ChatResponse`] for replay, carrying the recorded usage so
    /// cost/budget accounting runs deterministically.
    pub fn to_response(&self) -> ChatResponse {
        ChatResponse {
            content: self.content.clone(),
            role: self.role.clone(),
            model: self.model.clone(),
            tool_calls: self.tool_calls.clone(),
            usage: Usage {
                prompt_tokens: self.prompt_tokens,
                completion_tokens: self.completion_tokens,
                model: self.model.clone(),
                cache_read_input_tokens: self.cache_read_input_tokens,
                cache_creation_input_tokens: self.cache_creation_input_tokens,
            },
            stop_reason: self.stop_reason.clone(),
        }
    }
}

/// An in-memory tape (NDJSON on disk: one [`TapeEntry`] per line, diffable and
/// appendable). Keyed lookup serves the first entry recorded under a key.
#[derive(Debug, Default)]
pub struct Tape {
    entries: Vec<TapeEntry>,
}

impl Tape {
    /// Load a tape from an NDJSON file. A missing file yields an empty tape (the
    /// normal first-run/record case). Malformed lines are skipped.
    pub fn load(path: &Path) -> Tape {
        let mut tape = Tape::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<TapeEntry>(line) {
                    tape.entries.push(entry);
                }
            }
        }
        tape
    }

    /// Serialize the tape as NDJSON, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut out = String::new();
        for entry in &self.entries {
            out.push_str(&serde_json::to_string(entry).unwrap_or_default());
            out.push('\n');
        }
        std::fs::write(path, out)
    }

    /// First entry recorded under `key`, if any.
    pub fn lookup(&self, key: &str) -> Option<&TapeEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    pub fn has(&self, key: &str) -> bool {
        self.entries.iter().any(|e| e.key == key)
    }

    pub fn record(&mut self, entry: TapeEntry) {
        self.entries.push(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A loaded cassette: mode + tape + the file it flushes to.
#[derive(Debug)]
pub struct Cassette {
    pub mode: CassetteMode,
    pub path: PathBuf,
    pub tape: Tape,
    /// Whether the tape gained entries since load (so save is meaningful).
    pub dirty: bool,
}

/// What a call should do, decided up front so the tape borrow is never held across
/// the (possibly re-entrant) real provider call. The kind-specific payload (response,
/// stream chunks, or embeddings) is read off the returned [`TapeEntry`] by the caller.
pub enum Decision {
    /// Serve this recorded entry (replay / auto-hit). Boxed — a `TapeEntry` is large
    /// relative to the other variants.
    Replay(Box<TapeEntry>),
    /// `:replay` mode with no matching entry — a hard error (surfaces prompt drift).
    Miss(String),
    /// Call the real provider, then record the result.
    Record,
}

impl Cassette {
    pub fn load(path: PathBuf, mode: CassetteMode) -> Cassette {
        let tape = Tape::load(&path);
        Cassette {
            mode,
            path,
            tape,
            dirty: false,
        }
    }

    /// Decide how to handle a call for `key` based on mode + tape contents. Used by
    /// the complete, stream, and embed seams alike.
    pub fn decide(&self, key: &str) -> Decision {
        match self.mode {
            CassetteMode::Replay => match self.tape.lookup(key) {
                Some(e) => Decision::Replay(Box::new(e.clone())),
                None => Decision::Miss(key.to_string()),
            },
            CassetteMode::Auto => match self.tape.lookup(key) {
                Some(e) => Decision::Replay(Box::new(e.clone())),
                None => Decision::Record,
            },
            CassetteMode::Record => Decision::Record,
        }
    }

    /// Append a recorded interaction (complete / stream / embed) to the tape.
    pub fn record_entry(&mut self, entry: TapeEntry) {
        self.tape.record(entry);
        self.dirty = true;
    }

    /// Flush the tape to disk — but only if it gained entries. A replay-only
    /// session records nothing (`dirty` stays false), so `save`/`eject` won't
    /// rewrite the file and silently drop any tape line `Tape::load` couldn't
    /// parse.
    pub fn save(&self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.tape.save(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(content: &str, prompt: u32, completion: u32) -> ChatResponse {
        ChatResponse {
            content: content.to_string(),
            role: "assistant".to_string(),
            model: "fake-model".to_string(),
            tool_calls: vec![],
            usage: Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                model: "fake-model".to_string(),
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
            stop_reason: Some("end".to_string()),
        }
    }

    #[test]
    fn mcp_call_round_trips_through_the_tape() {
        let path = std::env::temp_dir().join(format!(
            "sema-cassette-mcp-{}-{}/tape.ndjson",
            std::process::id(),
            line!()
        ));
        let result = serde_json::json!({"content": [{"type": "text", "text": "hi"}]});

        let mut rec = Cassette::load(path.clone(), CassetteMode::Record);
        rec.record_entry(TapeEntry::from_mcp_call("mcpkey", &result));
        rec.save().unwrap();

        // Reloaded from disk in replay mode → serves the recorded result.
        let replay = Cassette::load(path, CassetteMode::Replay);
        match replay.decide("mcpkey") {
            Decision::Replay(entry) => {
                assert_eq!(entry.kind, "mcp-call");
                assert_eq!(entry.mcp_result.as_ref(), Some(&result));
            }
            _ => panic!("expected a replay hit for the recorded mcp-call"),
        }
        assert!(matches!(replay.decide("absent"), Decision::Miss(_)));
    }

    #[test]
    fn entry_round_trips_response_with_usage() {
        let r = resp("hello", 12, 34);
        let e = TapeEntry::from_response("k1", &r);
        let back = e.to_response();
        assert_eq!(back.content, "hello");
        assert_eq!(back.usage.prompt_tokens, 12);
        assert_eq!(back.usage.completion_tokens, 34);
        assert_eq!(back.model, "fake-model");
    }

    #[test]
    fn replay_mode_misses_are_hard_errors() {
        let cass = Cassette {
            mode: CassetteMode::Replay,
            path: PathBuf::from("/tmp/none.jsonl"),
            tape: Tape::default(),
            dirty: false,
        };
        assert!(matches!(cass.decide("missing"), Decision::Miss(_)));
    }

    #[test]
    fn auto_records_on_miss_then_replays() {
        let mut cass = Cassette {
            mode: CassetteMode::Auto,
            path: PathBuf::from("/tmp/none.jsonl"),
            tape: Tape::default(),
            dirty: false,
        };
        assert!(matches!(cass.decide("k"), Decision::Record));
        cass.record_entry(TapeEntry::from_response("k", &resp("recorded", 5, 6)));
        match cass.decide("k") {
            Decision::Replay(e) => {
                let r = e.to_response();
                assert_eq!(r.content, "recorded");
                assert_eq!(r.usage.completion_tokens, 6);
            }
            _ => panic!("expected replay after record"),
        }
    }

    #[test]
    fn tape_ndjson_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("sema-cassette-test-{}", std::process::id()));
        let path = dir.join("tape.jsonl");
        let mut tape = Tape::default();
        tape.record(TapeEntry::from_response("a", &resp("one", 1, 2)));
        tape.record(TapeEntry::from_response("b", &resp("two", 3, 4)));
        tape.save(&path).unwrap();

        let loaded = Tape::load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.lookup("b").unwrap().content, "two");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
