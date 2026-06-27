//! Canonical structured documentation for Sema's builtins and special forms.
//!
//! Each builtin/special form is one markdown file (`crates/sema-docs/entries/stdlib/<module>/<slug>.md`)
//! with a single YAML frontmatter block (`name`, `params`, `returns`, `see_also`, ...) followed by
//! a markdown body that may contain `sema` example code blocks.
//!
//! The filename is just a slug; the `name` field is canonical (so operator names like `*`, `<=`,
//! `null?` are fine). From this source `sema-docs gen` produces a single committed JSON index
//! ([`builtin_index`]) consumed at runtime by the LSP (hover/completion) and the REPL (apropos/doc).
//! The website is intentionally **not** generated from this yet.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

/// A single documented parameter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// Frontmatter as authored at the top of each entry file.
#[derive(Debug, Clone, Default, Deserialize)]
struct Frontmatter {
    name: String,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    section: Option<String>,
    #[serde(default)]
    params: Vec<Param>,
    #[serde(default)]
    returns: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    see_also: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
    /// Explicit summary; if absent it's derived from the first body paragraph.
    #[serde(default)]
    summary: Option<String>,
    /// Syntax template for special forms (e.g. `(let ((name value) ...) body ...)`).
    /// When present, shown as a signature block in hover and used as the label in
    /// signature help. Overrides flat parameter rendering for forms with complex syntax.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    syntax: Option<String>,
}

/// A fully resolved documentation entry (the serialized contract shared with LSP/REPL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub module: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub deprecated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub see_also: Vec<String>,
    /// Runnable example snippets (the contents of ```sema fenced blocks in the body).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// The full markdown body (including examples), used verbatim for hover.
    pub body: String,
    /// Syntax template for special forms (e.g. `(let ((name value) ...) body ...)`).
    /// When present, shown as a signature block in hover and used as the label in
    /// signature help. Overrides flat parameter rendering for forms with complex syntax.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<String>,
    /// `true` for special forms (no params schema; syntax lives in the body).
    #[serde(default, skip_serializing_if = "is_false")]
    pub special_form: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The serialized doc index (committed as JSON, loaded at runtime).
#[derive(Debug, Serialize, Deserialize)]
pub struct DocIndex {
    pub version: u32,
    pub entries: Vec<DocEntry>,
}

static BUILTIN_INDEX: OnceLock<DocIndex> = OnceLock::new();

/// Load the committed doc index that's compiled into the binary. Used by the LSP and REPL.
///
/// The JSON is deserialized once on first call and cached in a `OnceLock` —
/// subsequent calls (e.g. each `,apropos` in the REPL) return the cached
/// reference without re-parsing the ~11K-line JSON.
pub fn builtin_index() -> &'static DocIndex {
    BUILTIN_INDEX.get_or_init(|| {
        const JSON: &str = include_str!("../builtin_docs.generated.json");
        serde_json::from_str(JSON).expect("crates/sema-docs/builtin_docs.generated.json is valid")
    })
}

#[derive(Debug)]
pub struct DocError(pub String);
impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for DocError {}
type Result<T> = std::result::Result<T, DocError>;
fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(DocError(msg.into()))
}

// ── Parsing ───────────────────────────────────────────────────────

/// Split a leading `---\n...\n---` YAML frontmatter block; returns `(yaml, body)`.
fn split_frontmatter(text: &str) -> Result<(&str, &str)> {
    let t = text.trim_start_matches('\u{feff}');
    let t = t.trim_start_matches(['\n', '\r']);
    let rest = t
        .strip_prefix("---\n")
        .or_else(|| t.strip_prefix("---\r\n"))
        .ok_or_else(|| DocError("missing `---` frontmatter".into()))?;
    let end = rest
        .find("\n---")
        .ok_or_else(|| DocError("unterminated frontmatter".into()))?;
    let yaml = &rest[..end];
    let after = &rest[end + 4..];
    let after = after
        .strip_prefix('\n')
        .or_else(|| after.strip_prefix("\r\n"))
        .unwrap_or(after);
    Ok((yaml, after))
}

/// Parse a single entry file into a [`DocEntry`].
pub fn parse_entry(
    file: &Path,
    text: &str,
    default_module: &str,
    special_form: bool,
) -> Result<DocEntry> {
    let ctx = file.display();
    let (yaml, body_raw) = split_frontmatter(text).map_err(|e| DocError(format!("{ctx}: {e}")))?;
    let fm: Frontmatter =
        serde_yaml::from_str(yaml).map_err(|e| DocError(format!("{ctx}: frontmatter: {e}")))?;
    if fm.name.trim().is_empty() {
        return err(format!("{ctx}: missing `name`"));
    }
    let body = body_raw.trim().to_string();
    let summary = fm.summary.clone().unwrap_or_else(|| first_paragraph(&body));
    let examples = extract_sema_examples(&body);
    Ok(DocEntry {
        name: fm.name,
        aliases: fm.aliases,
        module: fm.module.unwrap_or_else(|| default_module.to_string()),
        section: fm.section,
        summary,
        params: fm.params,
        returns: fm.returns,
        since: fm.since,
        deprecated: fm.deprecated,
        see_also: fm.see_also,
        examples,
        body,
        syntax: fm.syntax,
        special_form,
    })
}

/// First prose paragraph of a markdown body, skipping any leading fenced code block (signature
/// blocks like ```sema\n(f x) → y\n``` are common at the top of an entry) and headings.
fn first_paragraph(body: &str) -> String {
    let mut lines = body.lines().peekable();
    // The first non-empty line of a leading signature block, used as a fallback summary
    // for entries whose body is ONLY a signature (e.g. `(f x) → y`) with no prose — so
    // such an entry still gets a non-empty summary rather than failing the strict gate.
    let mut signature_fallback = String::new();
    // Skip leading blank lines, leading fenced code blocks, and leading headings.
    loop {
        while matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
            lines.next();
        }
        match lines.peek() {
            Some(l) if l.trim_start().starts_with("```") => {
                lines.next(); // opening fence
                for l in lines.by_ref() {
                    if l.trim_start().starts_with("```") {
                        break;
                    }
                    if signature_fallback.is_empty() && !l.trim().is_empty() {
                        signature_fallback = l.trim().to_string();
                    }
                }
            }
            Some(l) if l.trim_start().starts_with('#') => {
                lines.next(); // leading heading — skip to the prose that follows
            }
            _ => break,
        }
    }
    let mut out = String::new();
    for line in lines {
        let l = line.trim();
        if l.is_empty() {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        if l.starts_with("```") || l.starts_with('#') {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(l);
    }
    if out.is_empty() {
        signature_fallback
    } else {
        out
    }
}

/// Extract the contents of ```sema fenced blocks from a markdown body.
fn extract_sema_examples(body: &str) -> Vec<String> {
    let mut examples = Vec::new();
    let mut in_block = false;
    let mut buf = String::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if !in_block && matches!(trimmed, "```sema" | "```scheme" | "```lisp") {
            in_block = true;
            buf.clear();
            continue;
        }
        if in_block && trimmed == "```" {
            in_block = false;
            let snippet = buf.trim_end().to_string();
            if !snippet.is_empty() {
                examples.push(snippet);
            }
            continue;
        }
        if in_block {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    examples
}

// ── Loading + validation ──────────────────────────────────────────

/// Recursively collect `*.md` files under `dir`.
fn collect_md(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    let rd = fs::read_dir(dir).map_err(|e| DocError(format!("reading {}: {e}", dir.display())))?;
    for entry in rd {
        let path = entry.map_err(|e| DocError(e.to_string()))?.path();
        if path.is_dir() {
            collect_md(&path, out)?;
        } else if path.extension().map(|x| x == "md").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

/// Load every entry from the stdlib doc tree and the special-forms tree.
pub fn load(stdlib_dir: &Path, special_forms_dir: &Path) -> Result<Vec<DocEntry>> {
    let mut entries = Vec::new();
    let mut load_tree = |root: &Path, special: bool| -> Result<()> {
        if !root.exists() {
            return Ok(());
        }
        let mut files = Vec::new();
        collect_md(root, &mut files)?;
        files.sort();
        for path in files {
            // default module = the immediate parent directory name relative to the tree.
            let default_module = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .filter(|d| Path::new(root).file_name().and_then(|s| s.to_str()) != Some(d))
                .unwrap_or(if special { "special-forms" } else { "misc" });
            let text = fs::read_to_string(&path)
                .map_err(|e| DocError(format!("reading {}: {e}", path.display())))?;
            entries.push(parse_entry(&path, &text, default_module, special)?);
        }
        Ok(())
    };
    load_tree(stdlib_dir, false)?;
    load_tree(special_forms_dir, true)?;
    entries.sort_by(|a, b| (&a.module, &a.name).cmp(&(&b.module, &b.name)));
    Ok(entries)
}

/// Validate the loaded entries. Hard errors (`Err`): duplicate (module, name) / (module, alias)
/// pairs, unbalanced code fences, leaked VitePress `:::` containers. Soft warnings (`Ok`): empty
/// summaries — these become hard errors under `strict` (the coverage gate).
pub fn validate(entries: &[DocEntry], strict: bool) -> Result<Vec<String>> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for e in entries {
        for n in std::iter::once(&e.name).chain(e.aliases.iter()) {
            let key = (e.module.clone(), n.clone());
            if !seen.insert(key) {
                errors.push(format!("duplicate doc name `{n}` in module `{}`", e.module));
            }
        }
        if e.summary.trim().is_empty() {
            let msg = format!("`{}` ({}) has an empty summary", e.name, e.module);
            if strict {
                errors.push(msg);
            } else {
                warnings.push(msg);
            }
        }
        if e.body.matches("```").count() % 2 != 0 {
            errors.push(format!(
                "`{}` ({}) has unbalanced ``` fences",
                e.name, e.module
            ));
        }
        if e.body.contains(":::") {
            errors.push(format!(
                "`{}` ({}) leaks a `:::` container into hover",
                e.name, e.module
            ));
        }
    }
    if errors.is_empty() {
        Ok(warnings)
    } else {
        err(format!(
            "doc validation failed:\n  - {}",
            errors.join("\n  - ")
        ))
    }
}

/// Drop duplicate entries within the same module (first wins, in load order).
/// Returns one warning per drop.
pub fn dedupe(entries: &mut Vec<DocEntry>) -> Vec<String> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut warnings = Vec::new();
    entries.retain(|e| {
        let names: Vec<&String> = std::iter::once(&e.name).chain(e.aliases.iter()).collect();
        // Report the SPECIFIC name/alias that collided. A canonical-name-vs-other-entry's
        // ALIAS clash would otherwise be reported only by `e.name`, hiding the real cause.
        if let Some(clash) = names
            .iter()
            .find(|n| seen.contains(&(e.module.clone(), n.to_string())))
        {
            let via = if **clash == e.name {
                String::new()
            } else {
                format!(" (via alias `{clash}`)")
            };
            warnings.push(format!(
                "dropped duplicate `{}`{via} in module `{}`",
                e.name, e.module
            ));
            false
        } else {
            for n in names {
                seen.insert((e.module.clone(), n.clone()));
            }
            true
        }
    });
    warnings
}

pub fn build_index(entries: Vec<DocEntry>) -> DocIndex {
    DocIndex {
        version: 1,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_index_is_cached() {
        // Two calls must return the same reference (OnceLock caching).
        let a = builtin_index();
        let b = builtin_index();
        assert!(
            std::ptr::eq(a, b),
            "builtin_index() should return a cached reference"
        );
    }

    #[test]
    fn builtin_index_has_entries() {
        let idx = builtin_index();
        assert!(!idx.entries.is_empty(), "doc index should have entries");
        // Every entry must have a name and module.
        for e in &idx.entries {
            assert!(!e.name.is_empty(), "entry has empty name");
            assert!(!e.module.is_empty(), "entry {} has empty module", e.name);
        }
    }
}
