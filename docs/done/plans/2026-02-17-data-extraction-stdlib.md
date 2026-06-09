# Data Extraction & Vision Stdlib Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add binary file I/O, file globbing, path utilities, base64 bytevector support, and multi-modal vision support to enable AI-powered data extraction pipelines (receipt OCR, PDF parsing, image analysis).

**Architecture:** Tier 1 adds foundational stdlib functions in `sema-stdlib` (binary I/O, path ops, glob). Tier 2 extends the LLM layer in `sema-llm` to support multi-modal content (images in messages), enabling vision models as OCR/extraction engines. The `ChatMessage.content` type changes from `String` to an enum supporting text and image content blocks, matching the Anthropic/OpenAI multi-modal APIs.

**Tech Stack:** Rust, `glob` crate (file globbing), `base64` (already a dep), Anthropic/OpenAI vision APIs (content blocks with `type: "image"` and base64 `source`)

---

## Dependency Map

```
Task 1: file/read-bytes, file/write-bytes     (sema-stdlib, standalone)
Task 2: base64/encode-bytes, base64/decode-bytes  (sema-stdlib, standalone)
Task 3: path/ utilities                        (sema-stdlib, standalone)
Task 4: file/glob                              (sema-stdlib, needs `glob` crate)
Task 5: Multi-modal ChatMessage content        (sema-core + sema-llm types)
Task 6: Anthropic vision support               (sema-llm, needs Task 5)
Task 7: OpenAI vision support                  (sema-llm, needs Task 5)
Task 8: llm/complete with images               (sema-llm builtins, needs Tasks 5-7)
Task 9: llm/extract-from-image                 (sema-llm builtins, needs Tasks 1-2, 8)

Tasks 1-4 are independent of each other.
Tasks 5-9 are sequential.
```

---

### Task 1: `file/read-bytes` and `file/write-bytes`

**Files:**

- Modify: `crates/sema-stdlib/src/io.rs`
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Write failing tests**

Add to `crates/sema/tests/integration_test.rs`:

```rust
#[test]
fn test_file_read_bytes() {
    let interp = Interpreter::new();
    // Write a known file, read it back as bytes
    let result = interp
        .eval_str(
            r#"(begin
                (file/write "/tmp/sema-test-bytes.txt" "ABC")
                (define bv (file/read-bytes "/tmp/sema-test-bytes.txt"))
                (list (bytevector-length bv)
                      (bytevector-u8-ref bv 0)
                      (bytevector-u8-ref bv 1)
                      (bytevector-u8-ref bv 2)))"#,
        )
        .unwrap();
    assert_eq!(result.to_string(), "(3 65 66 67)");
}

#[test]
fn test_file_read_bytes_not_found() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(file/read-bytes "/tmp/sema-nonexistent-xyz.bin")"#);
    assert!(result.is_err());
}

#[test]
fn test_file_write_bytes() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
                (file/write-bytes "/tmp/sema-test-write-bytes.bin" (bytevector 72 101 108 108 111))
                (file/read "/tmp/sema-test-write-bytes.bin"))"#,
        )
        .unwrap();
    assert_eq!(result.to_string(), "Hello");
}

#[test]
fn test_file_write_bytes_type_error() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(file/write-bytes "/tmp/foo.bin" "not a bytevector")"#);
    assert!(result.is_err());
}
```

**Step 2:** Run: `cargo test -p sema --test integration_test -- test_file_read_bytes test_file_write_bytes`
Expected: FAIL (functions not defined)

**Step 3: Implement**

Add to `crates/sema-stdlib/src/io.rs` in the `register` function, near the existing `file/read`:

```rust
crate::register_fn_gated(env, sandbox, Caps::FS_READ, "file/read-bytes", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("file/read-bytes", "1", args.len()));
    }
    let path = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let bytes = std::fs::read(path)
        .map_err(|e| SemaError::Io(format!("file/read-bytes {path}: {e}")))?;
    Ok(Value::bytevector(bytes))
});

crate::register_fn_gated(env, sandbox, Caps::FS_WRITE, "file/write-bytes", |args| {
    if args.len() != 2 {
        return Err(SemaError::arity("file/write-bytes", "2", args.len()));
    }
    let path = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let bv = args[1]
        .as_bytevector()
        .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
    std::fs::write(path, &*bv)
        .map_err(|e| SemaError::Io(format!("file/write-bytes {path}: {e}")))?;
    Ok(Value::nil())
});
```

**Step 4:** Run: `cargo test -p sema --test integration_test -- test_file_read_bytes test_file_write_bytes`
Expected: PASS

**Step 5:** Commit: `feat(stdlib): add file/read-bytes and file/write-bytes for binary I/O`

---

### Task 2: `base64/encode-bytes` and `base64/decode-bytes`

**Files:**

- Modify: `crates/sema-stdlib/src/crypto.rs`
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_base64_encode_bytes() {
    // "Hello" = [72 101 108 108 111] → "SGVsbG8="
    assert_eq!(
        eval(r#"(base64/encode-bytes (bytevector 72 101 108 108 111))"#),
        Value::string("SGVsbG8=")
    );
}

#[test]
fn test_base64_decode_bytes() {
    // "SGVsbG8=" → bytevector [72 101 108 108 111]
    let interp = Interpreter::new();
    let result = interp
        .eval_str(r#"(bytevector-length (base64/decode-bytes "SGVsbG8="))"#)
        .unwrap();
    assert_eq!(result, Value::int(5));
}

#[test]
fn test_base64_roundtrip_bytes() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
                (define bv (bytevector 0 1 255 128 64))
                (define encoded (base64/encode-bytes bv))
                (define decoded (base64/decode-bytes encoded))
                (= bv decoded))"#,
        )
        .unwrap();
    assert_eq!(result, Value::bool(true));
}

#[test]
fn test_base64_encode_bytes_type_error() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(base64/encode-bytes "not a bytevector")"#);
    assert!(result.is_err());
}

#[test]
fn test_base64_decode_bytes_invalid() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(base64/decode-bytes "!!!invalid!!!")"#);
    assert!(result.is_err());
}

#[test]
fn test_base64_encode_bytes_empty() {
    assert_eq!(
        eval(r#"(base64/encode-bytes (bytevector))"#),
        Value::string("")
    );
}
```

**Step 2:** Run: `cargo test -p sema --test integration_test -- test_base64`
Expected: FAIL

**Step 3: Implement**

Add to `crates/sema-stdlib/src/crypto.rs` after the existing `base64/decode`:

```rust
register_fn(env, "base64/encode-bytes", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("base64/encode-bytes", "1", args.len()));
    }
    let bv = args[0]
        .as_bytevector()
        .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&*bv);
    Ok(Value::string(&encoded))
});

register_fn(env, "base64/decode-bytes", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("base64/decode-bytes", "1", args.len()));
    }
    let s = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(|e| SemaError::eval(format!("base64/decode-bytes: {e}")))?;
    Ok(Value::bytevector(bytes))
});
```

**Step 4:** Run: `cargo test -p sema --test integration_test -- test_base64`
Expected: PASS

**Step 5:** Commit: `feat(stdlib): add base64/encode-bytes and base64/decode-bytes for bytevectors`

---

### Task 3: Path utilities

**Files:**

- Modify: `crates/sema-stdlib/src/io.rs`
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_path_ext() {
    assert_eq!(eval(r#"(path/ext "photo.jpg")"#), Value::string("jpg"));
    assert_eq!(eval(r#"(path/ext "archive.tar.gz")"#), Value::string("gz"));
    assert_eq!(eval(r#"(path/ext "Makefile")"#), Value::string(""));
    assert_eq!(eval(r#"(path/ext "/home/user/.bashrc")"#), Value::string(""));
}

#[test]
fn test_path_stem() {
    assert_eq!(eval(r#"(path/stem "photo.jpg")"#), Value::string("photo"));
    assert_eq!(eval(r#"(path/stem "/tmp/data.csv")"#), Value::string("data"));
    assert_eq!(eval(r#"(path/stem "Makefile")"#), Value::string("Makefile"));
}

#[test]
fn test_path_dir() {
    assert_eq!(eval(r#"(path/dir "/tmp/data.csv")"#), Value::string("/tmp"));
    assert_eq!(eval(r#"(path/dir "data.csv")"#), Value::string(""));
    assert_eq!(eval(r#"(path/dir "/home/user/.config/app.toml")"#), Value::string("/home/user/.config"));
}

#[test]
fn test_path_filename() {
    assert_eq!(eval(r#"(path/filename "/tmp/data.csv")"#), Value::string("data.csv"));
    assert_eq!(eval(r#"(path/filename "data.csv")"#), Value::string("data.csv"));
}

#[test]
fn test_path_join() {
    assert_eq!(eval(r#"(path/join "/tmp" "data.csv")"#), Value::string("/tmp/data.csv"));
    assert_eq!(eval(r#"(path/join "/home" "user" ".config")"#), Value::string("/home/user/.config"));
}

#[test]
fn test_path_absolute() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(path/absolute? "/tmp/data.csv")"#).unwrap();
    assert_eq!(result, Value::bool(true));
    let result = interp.eval_str(r#"(path/absolute? "data.csv")"#).unwrap();
    assert_eq!(result, Value::bool(false));
}
```

**Step 2:** Run: `cargo test -p sema --test integration_test -- test_path`
Expected: FAIL

**Step 3: Implement**

Add to `crates/sema-stdlib/src/io.rs`:

```rust
use std::path::Path;

register_fn(env, "path/ext", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("path/ext", "1", args.len()));
    }
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let ext = Path::new(p)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    Ok(Value::string(ext))
});

register_fn(env, "path/stem", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("path/stem", "1", args.len()));
    }
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let stem = Path::new(p)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    Ok(Value::string(stem))
});

register_fn(env, "path/dir", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("path/dir", "1", args.len()));
    }
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let dir = Path::new(p)
        .parent()
        .and_then(|d| d.to_str())
        .unwrap_or("");
    Ok(Value::string(dir))
});

register_fn(env, "path/filename", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("path/filename", "1", args.len()));
    }
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let name = Path::new(p)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    Ok(Value::string(name))
});

register_fn(env, "path/join", |args| {
    if args.is_empty() {
        return Err(SemaError::arity("path/join", "1+", 0));
    }
    let first = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let mut path = std::path::PathBuf::from(first);
    for arg in &args[1..] {
        let s = arg
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", arg.type_name()))?;
        path.push(s);
    }
    Ok(Value::string(path.to_str().unwrap_or("")))
});

register_fn(env, "path/absolute?", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("path/absolute?", "1", args.len()));
    }
    let p = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    Ok(Value::bool(Path::new(p).is_absolute()))
});
```

**Step 4:** Run: `cargo test -p sema --test integration_test -- test_path`
Expected: PASS

**Step 5:** Commit: `feat(stdlib): add path/ utilities (ext, stem, dir, filename, join, absolute?)`

---

### Task 4: `file/glob`

**Files:**

- Modify: `crates/sema-stdlib/Cargo.toml` — add `glob = "0.3"` dependency
- Modify: `crates/sema-stdlib/src/io.rs`
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Add dependency**

Add to `crates/sema-stdlib/Cargo.toml` under `[dependencies]`:

```toml
glob = "0.3"
```

**Step 2: Write failing tests**

```rust
#[test]
fn test_file_glob() {
    let interp = Interpreter::new();
    // Glob for Cargo.toml files in crates/
    let result = interp
        .eval_str(r#"(length (file/glob "crates/*/Cargo.toml"))"#)
        .unwrap();
    // We have 8 crates
    let count = result.as_int().unwrap();
    assert!(count >= 7, "expected at least 7 crate Cargo.toml files, got {count}");
}

#[test]
fn test_file_glob_no_matches() {
    assert_eq!(
        eval(r#"(file/glob "nonexistent-dir-xyz/*.nothing")"#).to_string(),
        "()"
    );
}

#[test]
fn test_file_glob_returns_list_of_strings() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(r#"(string? (car (file/glob "Cargo.*")))"#)
        .unwrap();
    assert_eq!(result, Value::bool(true));
}
```

**Step 3:** Run: `cargo test -p sema --test integration_test -- test_file_glob`
Expected: FAIL

**Step 4: Implement**

Add to `crates/sema-stdlib/src/io.rs`:

```rust
crate::register_fn_gated(env, sandbox, Caps::FS_READ, "file/glob", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("file/glob", "1", args.len()));
    }
    let pattern = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let paths = glob::glob(pattern)
        .map_err(|e| SemaError::eval(format!("file/glob: invalid pattern: {e}")))?;
    let items: Vec<Value> = paths
        .filter_map(|entry| entry.ok())
        .map(|path| Value::string(path.to_str().unwrap_or("")))
        .collect();
    Ok(Value::list(items))
});
```

**Step 5:** Run: `cargo test -p sema --test integration_test -- test_file_glob`
Expected: PASS

**Step 6:** Commit: `feat(stdlib): add file/glob for file pattern matching`

---

### Task 5: Multi-modal `ChatMessage` content type

**Problem:** Currently `ChatMessage.content` is a `String`. Vision APIs need content to be a list of content blocks:

```json
[
  {
    "type": "image",
    "source": { "type": "base64", "media_type": "image/png", "data": "..." }
  },
  { "type": "text", "text": "What's in this image?" }
]
```

**Files:**

- Modify: `crates/sema-llm/src/types.rs` — change `ChatMessage.content`
- Modify: `crates/sema-llm/src/anthropic.rs` — serialize content blocks
- Modify: `crates/sema-llm/src/openai.rs` — serialize content blocks
- Modify: `crates/sema-llm/src/ollama.rs` — serialize content blocks (if applicable)
- Modify: `crates/sema-llm/src/gemini.rs` — serialize content blocks
- Modify: `crates/sema-llm/src/builtins.rs` — update all content extraction to use new type
- Test: `crates/sema/tests/integration_test.rs`

**Step 1: Define content block types**

In `crates/sema-llm/src/types.rs`, add:

```rust
/// A content block in a chat message — either text or an image.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        /// Base64-encoded image data
        data: String,
    },
}

/// Message content: either a simple string or multi-modal content blocks.
#[derive(Debug, Clone)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            MessageContent::Blocks(blocks) => {
                // If there's exactly one text block, return it
                if blocks.len() == 1 {
                    if let ContentBlock::Text { text } = &blocks[0] {
                        return Some(text);
                    }
                }
                None
            }
        }
    }

    /// Get the text content, concatenating if needed.
    pub fn to_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn has_images(&self) -> bool {
        match self {
            MessageContent::Text(_) => false,
            MessageContent::Blocks(blocks) => blocks.iter().any(|b| matches!(b, ContentBlock::Image { .. })),
        }
    }
}
```

**Step 2: Update `ChatMessage`**

```rust
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
}
```

Remove the old `Serialize`/`Deserialize` derives — serialization is now manual per-provider because Anthropic and OpenAI serialize images differently.

Add convenience constructors:

```rust
impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(content.into()),
        }
    }

    pub fn with_blocks(role: impl Into<String>, blocks: Vec<ContentBlock>) -> Self {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Blocks(blocks),
        }
    }
}
```

**Step 3: Update all `ChatMessage { role: ..., content: ... }` construction sites**

Grep for `ChatMessage {` and `content:` in `sema-llm/src/`. Every site that constructs a `ChatMessage` with a `String` content needs to use `ChatMessage::new(role, content)` instead. Every site that reads `.content` as a string needs to use `.content.to_text()`.

This is a large mechanical migration. Key files:

- `builtins.rs` — the `extract_messages` function (line ~2454) constructs `ChatMessage` from Sema values. The `complete_with_prompt` function builds messages from `Prompt`.
- `anthropic.rs` — serializes messages to JSON. Must serialize `MessageContent::Blocks` as an array of content blocks.
- `openai.rs` — serializes messages. OpenAI uses `content: [{"type": "text", ...}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}]`.
- `ollama.rs` — serializes messages. Ollama uses `images: ["base64..."]` as a separate field.
- `gemini.rs` — serializes messages. Gemini uses `parts: [{"text": "..."}, {"inlineData": {"mimeType": "image/png", "data": "..."}}]`.

**Step 4:** Run: `cargo test -p sema-llm` and `cargo test -p sema --test integration_test`
Expected: PASS (all existing tests should work — text-only messages go through `MessageContent::Text` path)

**Step 5:** Commit: `feat(llm): add multi-modal MessageContent type for vision support`

---

### Task 6: Anthropic vision serialization

**Files:**

- Modify: `crates/sema-llm/src/anthropic.rs`

**Step 1:** Update the message serialization in `complete_async` to handle `MessageContent::Blocks`:

For Anthropic, when content has images, serialize as:

```json
{
  "role": "user",
  "content": [
    {
      "type": "image",
      "source": { "type": "base64", "media_type": "image/png", "data": "..." }
    },
    { "type": "text", "text": "What's in this image?" }
  ]
}
```

For text-only messages, keep the existing `"content": "string"` format.

**Step 2:** Run: `cargo build -p sema-llm`
Expected: compiles

**Step 3:** Commit: `feat(llm): Anthropic vision serialization support`

---

### Task 7: OpenAI vision serialization

**Files:**

- Modify: `crates/sema-llm/src/openai.rs`

**Step 1:** Update the message serialization in `complete_async` to handle `MessageContent::Blocks`:

For OpenAI, when content has images, serialize as:

```json
{
  "role": "user",
  "content": [
    {
      "type": "image_url",
      "image_url": { "url": "data:image/png;base64,..." }
    },
    { "type": "text", "text": "What's in this image?" }
  ]
}
```

**Step 2:** Run: `cargo build -p sema-llm`
Expected: compiles

**Step 3:** Commit: `feat(llm): OpenAI vision serialization support`

---

### Task 8: `llm/complete` with image support + `message/with-image`

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Modify: `crates/sema-core/src/value.rs` (update `Message` struct)
- Test: `crates/sema/tests/integration_test.rs`

**Step 1:** Update `Message` in `crates/sema-core/src/value.rs` to support multi-modal content:

```rust
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Optional image attachments (base64-encoded).
    pub images: Vec<ImageAttachment>,
}

#[derive(Debug, Clone)]
pub struct ImageAttachment {
    pub data: String, // base64
    pub media_type: String, // e.g. "image/png"
}
```

**Step 2:** Add `message/with-image` builtin in `crates/sema-llm/src/builtins.rs`:

```scheme
;; Usage:
;; (message/with-image :user "What's in this image?" (file/read-bytes "photo.jpg"))
;; (message/with-image :user "Describe" (file/read-bytes "photo.png") {:media-type "image/png"})
```

The function:

1. Takes role, text, bytevector, and optional options map
2. Base64-encodes the bytevector
3. Auto-detects media type from magic bytes (PNG: `\x89PNG`, JPEG: `\xFF\xD8\xFF`, GIF: `GIF8`, WebP: `RIFF....WEBP`, PDF: `%PDF`)
4. Returns a `Message` value with the image attached

**Step 3: Write tests**

```rust
#[test]
fn test_message_with_image_creates_message() {
    let interp = Interpreter::new();
    // Create a minimal PNG-like bytevector (just testing the plumbing, not a real image)
    let result = interp
        .eval_str(
            r#"(begin
                (define msg (message/with-image :user "Describe this" (bytevector 137 80 78 71)))
                (message? msg))"#,
        )
        .unwrap();
    assert_eq!(result, Value::bool(true));
}

#[test]
fn test_message_with_image_role() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
                (define msg (message/with-image :user "What is this?" (bytevector 1 2 3)))
                (:role msg))"#,
        )
        .unwrap();
    assert_eq!(result.to_string(), "user");
}
```

**Step 4:** Update `extract_messages` in `builtins.rs` to convert `Message.images` into `ContentBlock::Image` blocks when building the `ChatMessage`.

**Step 5:** Run: `cargo test -p sema --test integration_test`
Expected: PASS

**Step 6:** Commit: `feat(llm): add message/with-image for multi-modal messages`

---

### Task 9: `llm/extract-from-image` convenience function

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Test: `crates/sema/tests/integration_test.rs`

**Step 1:** This is a high-level convenience function. Implement in `builtins.rs`:

```scheme
;; Usage:
;; (llm/extract-from-image {:vendor :string :total :number} "receipt.jpg")
;; (llm/extract-from-image {:vendor :string :total :number} (file/read-bytes "receipt.jpg"))
;; (llm/extract-from-image schema path-or-bytes {:model "claude-sonnet-4-20250514"})
```

The function:

1. Accepts a schema (map), a source (string path or bytevector), and optional options
2. If source is a string, reads the file as bytes via `std::fs::read`
3. Base64-encodes the bytes
4. Auto-detects media type from magic bytes
5. Builds a multi-modal `ChatMessage` with the image + the extraction prompt (reusing the same schema-to-prompt logic from `llm/extract`)
6. Sends to the configured vision model
7. Parses the JSON response and validates against the schema (same as `llm/extract`)

**Step 2: Write tests**

```rust
#[test]
fn test_extract_from_image_arity() {
    let interp = Interpreter::new();
    // Too few args
    let result = interp.eval_str(r#"(llm/extract-from-image {:x :string})"#);
    assert!(result.is_err());
}

#[test]
fn test_extract_from_image_invalid_path() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(llm/extract-from-image {:x :string} "/nonexistent/file.png")"#);
    assert!(result.is_err());
}
```

Note: Full end-to-end tests require API keys and real images. Add a `#[ignore]` test for manual verification:

```rust
#[test]
#[ignore] // requires ANTHROPIC_API_KEY and a real image file
fn test_extract_from_image_e2e() {
    let interp = Interpreter::new();
    // Create a simple test image (1x1 red PNG) as bytevector
    let result = interp
        .eval_str(
            r#"(llm/extract-from-image
                {:description :string}
                (bytevector 137 80 78 71 13 10 26 10)  ; PNG header only — will fail but tests plumbing
                {:model "claude-sonnet-4-20250514"})"#,
        );
    // We just test it doesn't panic — actual extraction needs a real image + API key
    println!("Result: {:?}", result);
}
```

**Step 3:** Run: `cargo test -p sema --test integration_test -- test_extract_from_image`
Expected: arity and path tests PASS, e2e test IGNORED

**Step 4:** Commit: `feat(llm): add llm/extract-from-image for vision-based data extraction`

---

## Media Type Detection Helper

Used by Tasks 8 and 9. Implement as a private function in `builtins.rs`:

```rust
/// Detect media type from file magic bytes.
fn detect_media_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"%PDF") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}
```

Test:

```rust
#[test]
fn test_detect_media_type() {
    // This tests the auto-detection logic via message/with-image
    let interp = Interpreter::new();
    // PNG magic bytes: 0x89 0x50 0x4E 0x47
    let result = interp
        .eval_str(r#"(detect-media-type (bytevector 137 80 78 71 13 10 26 10))"#);
    // If we don't expose it, test via message/with-image options
}
```

If `detect_media_type` is not exposed as a Sema function, test it through `message/with-image` behavior or as a Rust unit test in `builtins.rs`.

---

## Summary of New Functions

| Function                 | Module | Signature                                      | Returns         |
| ------------------------ | ------ | ---------------------------------------------- | --------------- |
| `file/read-bytes`        | io     | `(file/read-bytes path)`                       | bytevector      |
| `file/write-bytes`       | io     | `(file/write-bytes path bv)`                   | nil             |
| `base64/encode-bytes`    | crypto | `(base64/encode-bytes bv)`                     | string          |
| `base64/decode-bytes`    | crypto | `(base64/decode-bytes str)`                    | bytevector      |
| `path/ext`               | io     | `(path/ext path)`                              | string          |
| `path/stem`              | io     | `(path/stem path)`                             | string          |
| `path/dir`               | io     | `(path/dir path)`                              | string          |
| `path/filename`          | io     | `(path/filename path)`                         | string          |
| `path/join`              | io     | `(path/join parts...)`                         | string          |
| `path/absolute?`         | io     | `(path/absolute? path)`                        | bool            |
| `file/glob`              | io     | `(file/glob pattern)`                          | list of strings |
| `message/with-image`     | llm    | `(message/with-image role text bytes)`         | message         |
| `llm/extract-from-image` | llm    | `(llm/extract-from-image schema source opts?)` | map             |

## Total Test Count

- Task 1: 4 tests (read-bytes, read-bytes-not-found, write-bytes, write-bytes-type-error)
- Task 2: 6 tests (encode, decode, roundtrip, type-error, invalid, empty)
- Task 3: 7 tests (ext, stem, dir, filename, join, absolute? × 2 cases)
- Task 4: 3 tests (glob, no-matches, returns-strings)
- Task 5: 0 (type refactor, covered by existing tests passing)
- Task 6-7: 0 (serialization, covered by build + existing tests)
- Task 8: 2 tests (creates-message, role)
- Task 9: 3 tests (arity, invalid-path, e2e-ignored)

**Total: 25 new tests** + all existing 1,078 tests must continue to pass.

---

## End-to-End Example (What This Enables)

```scheme
;; Extract receipt data from a photo using Claude's vision
(with-budget {:max-cost-usd 0.50}
  (define receipt-schema
    {:vendor :string
     :date :string
     :total :number
     :items [{:name :string :amount :number}]})

  ;; Single image extraction
  (define result
    (llm/extract-from-image receipt-schema "receipt.jpg"))
  (println result)

  ;; Batch processing with glob
  (define results
    (map (fn (path)
           (llm/extract-from-image receipt-schema path))
         (file/glob "receipts/*.{jpg,png}")))

  ;; Export to CSV
  (file/write "receipts.csv" (csv/encode results)))
```
