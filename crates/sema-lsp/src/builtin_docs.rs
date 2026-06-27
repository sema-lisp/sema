//! Structured builtin/special-form documentation for the LSP.
//!
//! The doc content is the canonical structured source in the `sema-docs` crate, compiled into a
//! committed JSON index that `sema_docs::builtin_index()` deserializes. This module wraps that
//! index in a name→entry lookup (including aliases) and renders entries to LSP Markdown. The old
//! `parse_stdlib_md` regex over website markdown is gone.

use crate::helpers::extract_params_from_doc;
use sema_docs::DocEntry;
use std::collections::HashMap;
use std::rc::Rc;

/// Name → documentation lookup (canonical names and aliases both resolve to the same entry).
/// Entries are shared via `Rc` so an entry with N aliases is stored once, not cloned N times.
pub struct BuiltinDocs {
    by_name: HashMap<String, Rc<DocEntry>>,
}

impl BuiltinDocs {
    /// Load from the compiled doc index.
    pub fn load() -> Self {
        let index = sema_docs::builtin_index();
        let mut by_name = HashMap::with_capacity(index.entries.len() * 2);
        for e in &index.entries {
            let rc = Rc::new(e.clone());
            for alias in &e.aliases {
                by_name
                    .entry(alias.clone())
                    .or_insert_with(|| Rc::clone(&rc));
            }
            by_name.insert(e.name.clone(), rc);
        }
        BuiltinDocs { by_name }
    }

    /// An empty store (used by lightweight subprocess-dispatch state).
    pub fn empty() -> Self {
        BuiltinDocs {
            by_name: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Rc<DocEntry>> {
        self.by_name.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }
}

/// One-line signature, e.g. `(string/split s sep)`.
/// For special forms with a `syntax` template, returns that verbatim.
pub fn signature(e: &DocEntry) -> String {
    if let Some(syn) = &e.syntax {
        return syn.clone();
    }
    if e.params.is_empty() {
        format!("({})", e.name)
    } else {
        let params: Vec<&str> = e.params.iter().map(|p| p.name.as_str()).collect();
        format!("({} {})", e.name, params.join(" "))
    }
}

/// Parameter names for a builtin: the structured params if present, else parsed from the first
/// example in the body (preserves inlay-hint param names until params are authored everywhere).
///
/// Note: special forms are intentionally left without signature-help parameters. Their syntax
/// examples (e.g. `(let ((x 1) (y 2)) (+ x y))`) don't map to a flat parameter list, so the
/// fallback is the bare form name (e.g. `let`). This is an inherent limitation of syntax-based
/// forms rather than function calls.
pub fn param_names(e: &DocEntry) -> Option<Vec<String>> {
    if !e.params.is_empty() {
        return Some(e.params.iter().map(|p| p.name.clone()).collect());
    }
    extract_params_from_doc(&e.body, &e.name)
}

/// Render an entry to Markdown for hover / completion documentation.
pub fn render_markdown(e: &DocEntry) -> String {
    let mut md = String::new();

    // Signature header — show when we have structured params, a return type, or an explicit
    // syntax template (special forms). Otherwise `(name)` adds noise; the body usually shows a
    // real example anyway.
    if !e.params.is_empty() || e.returns.is_some() || e.syntax.is_some() {
        md.push_str("```sema\n");
        md.push_str(&signature(e));
        if let Some(ret) = &e.returns {
            md.push_str(" → ");
            md.push_str(ret);
        }
        md.push_str("\n```\n\n");
    }

    md.push_str(e.body.trim());

    // Parameter docs, if any carry descriptions.
    if e.params.iter().any(|p| p.doc.is_some()) {
        md.push_str("\n\n**Parameters:**\n");
        for p in &e.params {
            md.push_str(&format!(
                "\n- `{}`{}{}",
                p.name,
                p.ty.as_deref()
                    .map(|t| format!(" : {t}"))
                    .unwrap_or_default(),
                p.doc
                    .as_deref()
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default(),
            ));
        }
    }

    if e.deprecated {
        md.push_str("\n\n**Deprecated.**");
    }
    if !e.see_also.is_empty() {
        let links: Vec<String> = e.see_also.iter().map(|s| format!("`{s}`")).collect();
        md.push_str(&format!("\n\nSee also: {}", links.join(", ")));
    }
    if let Some(since) = &e.since {
        md.push_str(&format!("\n\n_Since {since}_"));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_loads_and_resolves_known_names() {
        let docs = BuiltinDocs::load();
        // A stdlib builtin and a special form should both be present.
        assert!(docs.contains("string/split"), "missing string/split");
        assert!(docs.contains("define"), "missing special form `define`");
    }

    #[test]
    fn renders_markdown_with_body() {
        let docs = BuiltinDocs::load();
        let e = docs.get("string/split").expect("string/split");
        let md = render_markdown(e);
        assert!(md.contains("Split"), "rendered: {md}");
    }

    #[test]
    fn signature_uses_params_when_present() {
        let e = DocEntry {
            name: "f".into(),
            aliases: vec![],
            module: "m".into(),
            section: None,
            summary: "s".into(),
            params: vec![
                sema_docs::Param {
                    name: "a".into(),
                    ty: None,
                    doc: None,
                },
                sema_docs::Param {
                    name: "b".into(),
                    ty: None,
                    doc: None,
                },
            ],
            returns: Some("int".into()),
            since: None,
            deprecated: false,
            see_also: vec![],
            examples: vec![],
            body: "Adds.".into(),
            syntax: None,
            special_form: false,
        };
        assert_eq!(signature(&e), "(f a b)");
        assert!(render_markdown(&e).contains("(f a b) → int"));
    }

    #[test]
    fn aliases_share_same_rc_entry() {
        let docs = BuiltinDocs::load();
        // Find an entry that has at least one alias.
        let mut found = false;
        let by_name: Vec<(&String, &Rc<DocEntry>)> = docs.by_name.iter().collect();
        for (name, entry) in &by_name {
            if !entry.aliases.is_empty() {
                // The canonical name and its first alias must point to the same Rc.
                let alias = &entry.aliases[0];
                let canonical = docs.get(name).expect("canonical name");
                let alias_entry = docs.get(alias).expect("alias name");
                assert!(
                    Rc::ptr_eq(canonical, alias_entry),
                    "alias `{alias}` and canonical `{name}` should share the same Rc<DocEntry>"
                );
                found = true;
                break;
            }
        }
        assert!(
            found,
            "expected at least one entry with an alias in the doc index"
        );
    }
}
