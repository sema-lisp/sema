//! `,apropos PATTERN` — find Sema bindings whose name matches a pattern.
//!
//! Searches three sources:
//!   1. Env bindings (user-defined + stdlib) via `Env::iter_bindings`
//!      (walked recursively up the parent chain).
//!   2. Special forms via `sema_eval::SPECIAL_FORM_NAMES`.
//!   3. Builtin docs via the canonical `sema_docs::builtin_index`.
//!
//! Match strategy is layered:
//!   - **Exact prefix** (`split` matches `split-at` first)
//!   - **Substring** (case-insensitive) — catches `string/split` for `split`
//!   - **Fuzzy** via `sema_core::error::suggest_similar` — catches typos
//!
//! Results are ranked by tier and then alphabetically within each tier.

use std::collections::{BTreeMap, BTreeSet};

use sema_core::Env;
use sema_eval::SPECIAL_FORM_NAMES;

use crate::colors;

const MAX_RESULTS: usize = 50;

pub fn run(env: &Env, pattern: &str) {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        println!("Usage: ,apropos <pattern>");
        return;
    }

    let needle = pattern.to_lowercase();
    // name → one-line summary, from the canonical sema-docs index (aliases included).
    let docs: std::collections::HashMap<String, String> = {
        let mut m = std::collections::HashMap::new();
        for e in &sema_docs::builtin_index().entries {
            for a in &e.aliases {
                m.insert(a.clone(), e.summary.clone());
            }
            m.insert(e.name.clone(), e.summary.clone());
        }
        m
    };

    // Gather every candidate name we know about, with a one-line summary.
    let mut summaries: BTreeMap<String, String> = BTreeMap::new();

    // Env bindings — walk parent chain.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    collect_env(env, &mut seen);
    for name in &seen {
        let summary = describe_env_name(env, name, &docs);
        summaries.insert(name.clone(), summary);
    }

    // Special forms (may overlap with env if some are exported).
    for &sf in SPECIAL_FORM_NAMES {
        summaries.entry(sf.to_string()).or_insert_with(|| {
            docs.get(sf)
                .map(|d| first_doc_line(d.as_str()))
                .unwrap_or_else(|| "special form".to_string())
        });
    }

    // Builtin docs (covers entries that aren't in env yet, e.g., docs for
    // forms that resolve at compile time).
    for (name, doc) in &docs {
        summaries
            .entry(name.clone())
            .or_insert_with(|| first_doc_line(doc.as_str()));
    }

    // Tier each name: 0 = prefix, 1 = substring, 2 = fuzzy.
    let mut tiered: Vec<(u8, String, String)> = Vec::new();
    for (name, summary) in &summaries {
        let lower = name.to_lowercase();
        let tier = if lower.starts_with(&needle) {
            0
        } else if lower.contains(&needle) {
            1
        } else {
            continue;
        };
        tiered.push((tier, name.clone(), summary.clone()));
    }

    // If prefix + substring yielded too few, augment with fuzzy matches.
    if tiered.len() < 3 {
        let candidates: Vec<&str> = summaries.keys().map(|s| s.as_str()).collect();
        if let Some(suggestion) = sema_core::error::suggest_similar(pattern, &candidates) {
            if !tiered.iter().any(|(_, n, _)| n == &suggestion) {
                let summary = summaries.get(&suggestion).cloned().unwrap_or_default();
                tiered.push((2, suggestion, summary));
            }
        }
    }

    tiered.sort_by(|(ta, na, _), (tb, nb, _)| ta.cmp(tb).then(na.cmp(nb)));
    tiered.truncate(MAX_RESULTS);

    if tiered.is_empty() {
        println!("(no matches for {pattern:?})");
        return;
    }

    let max_width = tiered
        .iter()
        .map(|(_, n, _)| n.len())
        .max()
        .unwrap_or(0)
        .min(28);

    for (_, name, summary) in &tiered {
        let summary = summary.trim();
        if summary.is_empty() {
            println!("  {}", colors::cyan(name));
        } else {
            println!(
                "  {:width$}  {}",
                colors::cyan(name),
                colors::dim(summary),
                width = max_width + visible_padding(name)
            );
        }
    }
    println!(
        "  ({} match{})",
        tiered.len(),
        if tiered.len() == 1 { "" } else { "es" }
    );
}

fn collect_env(env: &Env, out: &mut BTreeSet<String>) {
    env.iter_bindings(|spur, _| {
        out.insert(sema_core::resolve(spur));
    });
    if let Some(parent) = &env.parent {
        collect_env(parent, out);
    }
}

fn describe_env_name(
    env: &Env,
    name: &str,
    docs: &std::collections::HashMap<String, String>,
) -> String {
    // Try the doc table first — most expressive.
    if let Some(doc) = docs.get(name) {
        return first_doc_line(doc);
    }
    // Fall back to the value's type.
    let spur = sema_core::intern(name);
    match env.get(spur) {
        Some(val) if sema_vm::extract_vm_closure(&val).is_some() => "lambda".to_string(),
        Some(val) => val.type_name().to_string(),
        None => String::new(),
    }
}

/// First non-empty prose line of a markdown doc string, with backticks
/// stripped for terminal readability. Skips fenced code blocks entirely.
fn first_doc_line(doc: &str) -> String {
    let mut in_code = false;
    for line in doc.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || trimmed.is_empty() {
            continue;
        }
        return trimmed.replace('`', "");
    }
    String::new()
}

/// `colors::cyan` adds ANSI escapes that don't count toward visible width;
/// `format!` width-padding sees them as characters. Compensate by adding
/// the escape length to the field width when colour is enabled.
fn visible_padding(s: &str) -> usize {
    let coloured = colors::cyan(s);
    coloured.len().saturating_sub(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_doc_line_skips_code_fences() {
        let doc = "```sema\n(foo)\n```\nReturns the foo.";
        assert_eq!(first_doc_line(doc), "Returns the foo.");
    }

    #[test]
    fn first_doc_line_strips_backticks() {
        assert_eq!(first_doc_line("Use `bar` to baz."), "Use bar to baz.");
    }
}
