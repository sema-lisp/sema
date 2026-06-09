# P1 — Text Processing (Tasks 4-7)

> Parent plan: [../2026-02-17-llm-pipeline-stdlib.md](../2026-02-17-llm-pipeline-stdlib.md)

---

## Task 4: Text Chunking

**Files:**

- Create: `crates/sema-stdlib/src/text.rs`
- Modify: `crates/sema-stdlib/src/lib.rs` (add `mod text` + register call)
- Test: `crates/sema/tests/integration_test.rs`

### Overview

New `text.rs` module with recursive character-based text chunking. Functions:
`text/chunk` (recursive splitting by separator hierarchy), `text/chunk-by-separator`,
`text/split-sentences`. No new deps (regex already in workspace but not needed here).

### Step 1: Write failing tests

Add to `crates/sema/tests/integration_test.rs`:

```rust
#[test]
fn test_text_chunk_basic() {
    let result = eval(r#"(text/chunk "hello world foo bar" {:size 10})"#);
    let chunks = result.as_list().expect("should be a list");
    assert!(chunks.len() >= 2);
    for chunk in chunks {
        let s = chunk.as_str().expect("each chunk should be a string");
        assert!(s.len() <= 10, "chunk too long: '{s}' ({})", s.len());
    }
}

#[test]
fn test_text_chunk_default_size() {
    let result = eval(r#"(text/chunk "short text")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].as_str().unwrap(), "short text");
}

#[test]
fn test_text_chunk_with_overlap() {
    let result = eval(r#"(text/chunk "aaaa bbbb cccc dddd" {:size 10 :overlap 4})"#);
    let chunks = result.as_list().expect("should be a list");
    assert!(chunks.len() >= 2);
}

#[test]
fn test_text_chunk_empty() {
    let result = eval(r#"(text/chunk "")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 0);
}

#[test]
fn test_text_chunk_by_separator() {
    let result = eval(r#"(text/chunk-by-separator "a\nb\nc\nd" "\n")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 4);
    assert_eq!(chunks[0].as_str().unwrap(), "a");
}

#[test]
fn test_text_chunk_by_separator_empty() {
    let result = eval(r#"(text/chunk-by-separator "" "\n")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 0);
}

#[test]
fn test_text_split_sentences() {
    let result = eval(r#"(text/split-sentences "Hello world. How are you? I am fine.")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 3);
}

#[test]
fn test_text_split_sentences_empty() {
    let result = eval(r#"(text/split-sentences "")"#);
    assert_eq!(result.as_list().unwrap().len(), 0);
}

#[test]
fn test_text_split_sentences_no_punctuation() {
    let result = eval(r#"(text/split-sentences "hello world")"#);
    let chunks = result.as_list().unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].as_str().unwrap(), "hello world");
}
```

**Run:** `cargo test -p sema --test integration_test -- test_text_chunk test_text_split`
**Expected:** FAIL — functions not defined

### Step 2: Create `text.rs` with chunking functions

Create `crates/sema-stdlib/src/text.rs`:

```rust
use sema_core::{SemaError, Value};
use crate::register_fn;

pub fn register(env: &sema_core::Env) {
    // (text/chunk text) or (text/chunk text {:size 1000 :overlap 200})
    register_fn(env, "text/chunk", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("text/chunk", "1-2", args.len()));
        }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if text.is_empty() { return Ok(Value::list(vec![])); }

        let mut chunk_size: usize = 1000;
        let mut overlap: usize = 200;
        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            if let Some(v) = opts.get(&Value::keyword("size")).and_then(|v| v.as_int()) {
                chunk_size = v.max(1) as usize;
            }
            if let Some(v) = opts.get(&Value::keyword("overlap")).and_then(|v| v.as_int()) {
                overlap = v.max(0) as usize;
            }
        }
        if overlap >= chunk_size { overlap = 0; }
        let chunks = recursive_chunk(text, chunk_size, overlap);
        Ok(Value::list(chunks.into_iter().map(|s| Value::string(&s)).collect()))
    });

    // (text/chunk-by-separator text separator)
    register_fn(env, "text/chunk-by-separator", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("text/chunk-by-separator", "2", args.len()));
        }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sep = args[1].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        if text.is_empty() { return Ok(Value::list(vec![])); }
        let chunks: Vec<Value> = text.split(sep)
            .filter(|s| !s.is_empty())
            .map(|s| Value::string(s))
            .collect();
        Ok(Value::list(chunks))
    });

    // (text/split-sentences text)
    register_fn(env, "text/split-sentences", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("text/split-sentences", "1", args.len()));
        }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if text.is_empty() { return Ok(Value::list(vec![])); }
        let sentences = split_sentences(text);
        Ok(Value::list(sentences.into_iter().map(|s| Value::string(&s)).collect()))
    });
}

const SEPARATORS: &[&str] = &["\n\n", "\n", ". ", "! ", "? ", "; ", ", ", " "];

fn recursive_chunk(text: &str, max_size: usize, overlap: usize) -> Vec<String> {
    if text.len() <= max_size { return vec![text.to_string()]; }
    for sep in SEPARATORS {
        let parts: Vec<&str> = text.split(sep).collect();
        if parts.len() > 1 {
            return merge_splits(&parts, sep, max_size, overlap);
        }
    }
    hard_chunk(text, max_size, overlap)
}

fn merge_splits(parts: &[&str], sep: &str, max_size: usize, overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for part in parts {
        let with_sep = if current.is_empty() {
            part.to_string()
        } else {
            format!("{}{}{}", current, sep, part)
        };
        if with_sep.len() <= max_size {
            current = with_sep;
        } else {
            if !current.is_empty() { chunks.push(current.clone()); }
            if part.len() > max_size {
                chunks.extend(recursive_chunk(part, max_size, overlap));
                current = String::new();
            } else {
                current = part.to_string();
            }
        }
    }
    if !current.is_empty() { chunks.push(current); }
    if overlap > 0 && chunks.len() > 1 { apply_overlap(&chunks, overlap) } else { chunks }
}

fn apply_overlap(chunks: &[String], overlap: usize) -> Vec<String> {
    let mut result = vec![chunks[0].clone()];
    for i in 1..chunks.len() {
        let prev = &chunks[i - 1];
        let ov = if prev.len() > overlap { &prev[prev.len() - overlap..] } else { prev.as_str() };
        result.push(format!("{}{}", ov, chunks[i]));
    }
    result
}

fn hard_chunk(text: &str, max_size: usize, overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let step = if overlap < max_size { max_size - overlap } else { max_size };
    let mut i = 0;
    while i < chars.len() {
        let end = (i + max_size).min(chars.len());
        chunks.push(chars[i..end].iter().collect());
        i += step;
    }
    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        current.push(chars[i]);
        if (chars[i] == '.' || chars[i] == '!' || chars[i] == '?')
            && (i + 1 >= chars.len() || chars[i + 1].is_whitespace())
        {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() { sentences.push(trimmed); }
            current = String::new();
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() { sentences.push(trimmed); }
    sentences
}
```

### Step 3: Register the text module

In `crates/sema-stdlib/src/lib.rs`, add `mod text;` and call `text::register(env);` in `register_stdlib`.

### Step 4: Run tests

**Run:** `cargo test -p sema --test integration_test -- test_text_chunk test_text_split`
**Expected:** PASS

### Step 5: Commit

```bash
git add crates/sema-stdlib/src/text.rs crates/sema-stdlib/src/lib.rs crates/sema/tests/integration_test.rs
git commit -m "feat(stdlib): add text chunking module with recursive splitting"
```

---

## Task 5: Text Cleaning Utilities

**Files:**

- Modify: `crates/sema-stdlib/src/text.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Step 1: Write failing tests

```rust
#[test]
fn test_text_clean_whitespace() {
    assert_eq!(
        eval(r#"(text/clean-whitespace "  hello   world  \n\n  foo  ")"#),
        Value::string("hello world foo")
    );
}

#[test]
fn test_text_strip_html() {
    assert_eq!(
        eval(r#"(text/strip-html "<p>Hello <b>world</b></p>")"#),
        Value::string("Hello world")
    );
}

#[test]
fn test_text_strip_html_entities() {
    assert_eq!(
        eval(r#"(text/strip-html "a &amp; b &lt; c")"#),
        Value::string("a & b < c")
    );
}

#[test]
fn test_text_truncate_short() {
    assert_eq!(eval(r#"(text/truncate "hello" 10)"#), Value::string("hello"));
}

#[test]
fn test_text_truncate_exact() {
    assert_eq!(eval(r#"(text/truncate "hello world" 5)"#), Value::string("he..."));
}

#[test]
fn test_text_truncate_custom_suffix() {
    assert_eq!(eval(r#"(text/truncate "hello world" 8 "…")"#), Value::string("hello w…"));
}

#[test]
fn test_text_word_count() {
    assert_eq!(eval(r#"(text/word-count "hello world foo bar")"#), Value::int(4));
}

#[test]
fn test_text_word_count_empty() {
    assert_eq!(eval(r#"(text/word-count "")"#), Value::int(0));
}

#[test]
fn test_text_word_count_extra_spaces() {
    assert_eq!(eval(r#"(text/word-count "  hello   world  ")"#), Value::int(2));
}

#[test]
fn test_text_trim_indent() {
    assert_eq!(eval(r#"(text/trim-indent "    hello\n    world")"#), Value::string("hello\nworld"));
}

#[test]
fn test_text_trim_indent_mixed() {
    assert_eq!(eval(r#"(text/trim-indent "    hello\n      world")"#), Value::string("hello\n  world"));
}

#[test]
fn test_text_trim_indent_empty() {
    assert_eq!(eval(r#"(text/trim-indent "")"#), Value::string(""));
}
```

**Run:** `cargo test -p sema --test integration_test -- test_text_clean test_text_strip test_text_truncate test_text_word test_text_trim_indent`
**Expected:** FAIL

### Step 2: Implement text cleaning functions

Add inside `register()` in `text.rs`:

```rust
    register_fn(env, "text/clean-whitespace", |args| {
        if args.len() != 1 { return Err(SemaError::arity("text/clean-whitespace", "1", args.len())); }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&text.split_whitespace().collect::<Vec<_>>().join(" ")))
    });

    register_fn(env, "text/strip-html", |args| {
        if args.len() != 1 { return Err(SemaError::arity("text/strip-html", "1", args.len())); }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&strip_html(text)))
    });

    register_fn(env, "text/truncate", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("text/truncate", "2-3", args.len()));
        }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let max_len = args[1].as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))? as usize;
        let suffix = args.get(2).and_then(|v| v.as_str())
            .unwrap_or("...").to_string();
        let char_count = text.chars().count();
        if char_count <= max_len { return Ok(Value::string(text)); }
        let suffix_len = suffix.chars().count();
        if max_len <= suffix_len { return Ok(Value::string(&suffix)); }
        let take = max_len - suffix_len;
        let truncated: String = text.chars().take(take).collect();
        Ok(Value::string(&format!("{truncated}{suffix}")))
    });

    register_fn(env, "text/word-count", |args| {
        if args.len() != 1 { return Err(SemaError::arity("text/word-count", "1", args.len())); }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::int(text.split_whitespace().count() as i64))
    });

    register_fn(env, "text/trim-indent", |args| {
        if args.len() != 1 { return Err(SemaError::arity("text/trim-indent", "1", args.len())); }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&trim_indent(text)))
    });
```

Add helpers outside `register()`:

```rust
fn strip_html(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
        .replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn trim_indent(text: &str) -> String {
    if text.is_empty() { return String::new(); }
    let lines: Vec<&str> = text.split('\n').collect();
    let min_indent = lines.iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min().unwrap_or(0);
    lines.iter().map(|line| {
        if line.len() >= min_indent { &line[min_indent..] } else { line.trim_start() }
    }).collect::<Vec<_>>().join("\n")
}
```

### Step 3: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_text_clean test_text_strip test_text_truncate test_text_word test_text_trim`
**Expected:** PASS

```bash
git add crates/sema-stdlib/src/text.rs crates/sema/tests/integration_test.rs
git commit -m "feat(stdlib): add text cleaning — clean-whitespace, strip-html, truncate, word-count, trim-indent"
```

---

## Task 6: Prompt Templates

**Files:**

- Modify: `crates/sema-stdlib/src/text.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Step 1: Write failing tests

```rust
#[test]
fn test_prompt_template_basic() {
    let result = eval(r#"(prompt/template "Hello {{name}}")"#);
    assert!(result.as_str().is_some());
}

#[test]
fn test_prompt_render_basic() {
    assert_eq!(
        eval(r#"(prompt/render "Hello {{name}}, welcome to {{place}}." {:name "Alice" :place "Wonderland"})"#),
        Value::string("Hello Alice, welcome to Wonderland.")
    );
}

#[test]
fn test_prompt_render_missing_var() {
    assert_eq!(
        eval(r#"(prompt/render "Hello {{name}}, {{missing}}." {:name "Bob"})"#),
        Value::string("Hello Bob, {{missing}}.")
    );
}

#[test]
fn test_prompt_render_no_vars() {
    assert_eq!(eval(r#"(prompt/render "Hello world." {})"#), Value::string("Hello world."));
}

#[test]
fn test_prompt_render_number_value() {
    assert_eq!(eval(r#"(prompt/render "Count: {{n}}" {:n 42})"#), Value::string("Count: 42"));
}

#[test]
fn test_prompt_render_repeated_var() {
    assert_eq!(
        eval(r#"(prompt/render "{{x}} and {{x}}" {:x "hello"})"#),
        Value::string("hello and hello")
    );
}

#[test]
fn test_prompt_render_adjacent_vars() {
    assert_eq!(
        eval(r#"(prompt/render "{{a}}{{b}}" {:a "hello" :b "world"})"#),
        Value::string("helloworld")
    );
}
```

**Run:** `cargo test -p sema --test integration_test -- test_prompt`
**Expected:** FAIL

### Step 2: Implement prompt template functions

Add inside `register()` in `text.rs`:

```rust
    register_fn(env, "prompt/template", |args| {
        if args.len() != 1 { return Err(SemaError::arity("prompt/template", "1", args.len())); }
        let text = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(text))
    });

    register_fn(env, "prompt/render", |args| {
        if args.len() != 2 { return Err(SemaError::arity("prompt/render", "2", args.len())); }
        let template = args[0].as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let vars = args[1].as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        Ok(Value::string(&render_template(template, &vars)))
    });
```

Add helper:

```rust
fn render_template(template: &str, vars: &std::collections::BTreeMap<Value, Value>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'{') {
            chars.next();
            let mut var_name = String::new();
            let mut found_close = false;
            while let Some(c) = chars.next() {
                if c == '}' && chars.peek() == Some(&'}') {
                    chars.next();
                    found_close = true;
                    break;
                }
                var_name.push(c);
            }
            if found_close {
                if let Some(val) = vars.get(&Value::keyword(&var_name)) {
                    if let Some(s) = val.as_str() { result.push_str(s); }
                    else { result.push_str(&val.to_string()); }
                } else {
                    result.push_str("{{"); result.push_str(&var_name); result.push_str("}}");
                }
            } else {
                result.push_str("{{"); result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }
    result
}
```

### Step 3: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_prompt`
**Expected:** PASS

```bash
git add crates/sema-stdlib/src/text.rs crates/sema/tests/integration_test.rs
git commit -m "feat(stdlib): add prompt templates with mustache-style variable substitution"
```

---

## Task 7: Token Counting

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`
- Test: `crates/sema/tests/integration_test.rs`

### Step 1: Write failing tests

```rust
#[test]
fn test_llm_token_count_basic() {
    let result = eval(r#"(llm/token-count "hello world")"#);
    let count = result.as_int().expect("should be integer");
    assert!(count >= 2 && count <= 4, "unexpected count: {count}");
}

#[test]
fn test_llm_token_count_empty() {
    assert_eq!(eval(r#"(llm/token-count "")"#), Value::int(0));
}

#[test]
fn test_llm_token_count_long() {
    let result = eval(r#"(llm/token-count (string-append "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd"))"#);
    assert_eq!(result.as_int().unwrap(), 10); // 40 chars / 4
}

#[test]
fn test_llm_token_estimate_map() {
    let result = eval(r#"(llm/token-estimate "hello world")"#);
    let map = result.as_map_rc().expect("should be a map");
    assert!(map.contains_key(&Value::keyword("tokens")));
    assert!(map.contains_key(&Value::keyword("method")));
    assert_eq!(map.get(&Value::keyword("method")).unwrap().as_str().unwrap(), "chars/4");
}

#[test]
fn test_llm_token_count_list() {
    let result = eval(r#"(llm/token-count '("hello" "world" "foo"))"#);
    let count = result.as_int().expect("should be integer");
    assert!(count >= 3);
}
```

**Run:** `cargo test -p sema --test integration_test -- test_llm_token`
**Expected:** FAIL

### Step 2: Implement token counting

Add inside `register_llm_builtins()` in `builtins.rs`:

```rust
register_fn(env, "llm/token-count", |args| {
    if args.len() != 1 { return Err(SemaError::arity("llm/token-count", "1", args.len())); }
    let char_count = if let Some(s) = args[0].as_str() {
        s.len()
    } else if let Some(list) = args[0].as_list() {
        list.iter().map(|v| v.as_str().map(|s| s.len()).unwrap_or_else(|| v.to_string().len())).sum()
    } else {
        args[0].to_string().len()
    };
    Ok(Value::int((char_count / 4) as i64))
});

register_fn(env, "llm/token-estimate", |args| {
    if args.len() != 1 { return Err(SemaError::arity("llm/token-estimate", "1", args.len())); }
    let char_count = if let Some(s) = args[0].as_str() { s.len() }
    else { args[0].to_string().len() };
    let tokens = (char_count / 4) as i64;
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("tokens"), Value::int(tokens));
    map.insert(Value::keyword("method"), Value::string("chars/4"));
    map.insert(Value::keyword("chars"), Value::int(char_count as i64));
    Ok(Value::map(map))
});
```

### Step 3: Run tests, commit

**Run:** `cargo test -p sema --test integration_test -- test_llm_token`
**Expected:** PASS

```bash
git add crates/sema-llm/src/builtins.rs crates/sema/tests/integration_test.rs
git commit -m "feat(llm): add token counting with chars/4 estimate"
```
