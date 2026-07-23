use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crossterm::terminal;
use sema_docs::DocEntry;

use crate::colors;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PagerMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct AproposHit {
    pub name: String,
    pub summary: String,
}

static DOC_NAME_INDEX: OnceLock<HashMap<String, usize>> = OnceLock::new();

fn doc_name_index() -> &'static HashMap<String, usize> {
    DOC_NAME_INDEX.get_or_init(|| {
        let mut by_name = HashMap::new();
        for (idx, entry) in sema_docs::builtin_index().entries.iter().enumerate() {
            by_name.insert(entry.name.clone(), idx);
            for alias in &entry.aliases {
                by_name.entry(alias.clone()).or_insert(idx);
            }
        }
        by_name
    })
}

pub(crate) fn lookup(name: &str) -> Option<&'static DocEntry> {
    let idx = *doc_name_index().get(name)?;
    sema_docs::builtin_index().entries.get(idx)
}

pub(crate) fn completion_candidates(prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = doc_name_index()
        .keys()
        .filter(|name| name.starts_with(prefix))
        .cloned()
        .collect();
    out.sort();
    out
}

pub(crate) fn rendered_doc(name: &str) -> Option<String> {
    let entry = lookup(name)?;
    Some(rendered_doc_entry(name, entry))
}

pub(crate) fn rendered_doc_entry(name: &str, entry: &DocEntry) -> String {
    let kind = if entry.special_form {
        "special form"
    } else {
        "builtin"
    };
    let heading = if name == entry.name {
        format!("{} {} {}", cyan(&entry.name), dim(":"), kind)
    } else {
        format!(
            "{} {} {} {} {}",
            cyan(name),
            dim("→"),
            cyan(&entry.name),
            dim(":"),
            kind
        )
    };
    let md = sema_lsp::builtin_docs::render_markdown(entry);
    // Blank line between the heading and the body so the doc doesn't read as
    // one cramped block (matches Julia/Elixir/Python REPL doc rendering).
    // trim_start guards against a double gap if the rendered body already
    // leads with whitespace.
    let body = render_terminal_markdown(&md);
    format!("{heading}\n\n{}\n", body.trim_start_matches('\n'))
}

pub(crate) fn print_rendered(text: &str, pager: PagerMode) -> io::Result<()> {
    if should_page(text, pager) && page_with_less(text)? {
        return Ok(());
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()
}

fn should_page(text: &str, pager: PagerMode) -> bool {
    match pager {
        PagerMode::Never => false,
        PagerMode::Always => io::stdout().is_terminal(),
        PagerMode::Auto => {
            if !io::stdout().is_terminal() {
                return false;
            }
            let Ok((_, rows)) = terminal::size() else {
                return false;
            };
            text.lines().count() > rows as usize
        }
    }
}

fn page_with_less(text: &str) -> io::Result<bool> {
    let mut child = match Command::new("less")
        .arg("-R")
        .arg("-F")
        .arg("-X")
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let _ = child.wait()?;
    Ok(true)
}

pub(crate) fn doc_search_results(
    query: &str,
    limit: usize,
) -> Vec<sema_mcp::docs_search::SearchHit> {
    sema_mcp::docs_search::search(query, limit)
}

pub(crate) fn render_search_results(
    query: &str,
    hits: &[sema_mcp::docs_search::SearchHit],
) -> String {
    if hits.is_empty() {
        return format!("(no documentation matches for {query:?})\n");
    }

    let max_width = hits.iter().map(|h| h.name.len()).max().unwrap_or(0).min(28);
    let mut out = String::new();
    for hit in hits {
        out.push_str(&format!(
            "  {:width$}  {}  {}\n",
            cyan(&hit.name),
            dim(&format!("[{}]", hit.module)),
            style_inline(hit.summary.trim()),
            width = max_width + visible_padding(&hit.name)
        ));
    }
    out.push_str(&format!(
        "  ({} match{})\n",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" }
    ));
    out
}

pub(crate) fn builtin_apropos_hits(pattern: &str) -> Vec<AproposHit> {
    let mut summaries: BTreeMap<String, String> = BTreeMap::new();
    for entry in &sema_docs::builtin_index().entries {
        let summary = first_doc_line(&entry.summary);
        summaries
            .entry(entry.name.clone())
            .or_insert_with(|| summary.clone());
        for alias in &entry.aliases {
            summaries
                .entry(alias.clone())
                .or_insert_with(|| summary.clone());
        }
    }
    search_name_summaries(pattern, &summaries)
}

pub(crate) fn search_name_summaries(
    pattern: &str,
    summaries: &BTreeMap<String, String>,
) -> Vec<AproposHit> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Vec::new();
    }

    let needle = pattern.to_lowercase();
    let mut tiered: Vec<(u8, String, String)> = Vec::new();
    for (name, summary) in summaries {
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

    if tiered.len() < 3 {
        let candidates: Vec<&str> = summaries.keys().map(|s| s.as_str()).collect();
        if let Some(suggestion) = sema_core::error::suggest_similar(pattern, &candidates) {
            if !tiered.iter().any(|(_, name, _)| name == &suggestion) {
                let summary = summaries.get(&suggestion).cloned().unwrap_or_default();
                tiered.push((2, suggestion, summary));
            }
        }
    }

    tiered.sort_by(|(ta, na, _), (tb, nb, _)| ta.cmp(tb).then(na.cmp(nb)));
    tiered.truncate(50);
    tiered
        .into_iter()
        .map(|(_, name, summary)| AproposHit { name, summary })
        .collect()
}

pub(crate) fn render_apropos_hits(pattern: &str, hits: &[AproposHit]) -> String {
    if hits.is_empty() {
        return format!("(no matches for {pattern:?})\n");
    }

    let max_width = hits.iter().map(|h| h.name.len()).max().unwrap_or(0).min(28);
    let mut out = String::new();
    for hit in hits {
        let summary = hit.summary.trim();
        if summary.is_empty() {
            out.push_str(&format!("  {}\n", colors::cyan(&hit.name)));
        } else {
            out.push_str(&format!(
                "  {:width$}  {}\n",
                cyan(&hit.name),
                dim(summary),
                width = max_width + visible_padding(&hit.name)
            ));
        }
    }
    out.push_str(&format!(
        "  ({} match{})\n",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" }
    ));
    out
}

pub(crate) fn render_terminal_markdown(md: &str) -> String {
    render_terminal_markdown_inner(md, colors::enabled_stdout())
}

/// Rendering core with an explicit color decision so tests can exercise the
/// styled path without a TTY (`render_terminal_markdown` supplies the real
/// `colors::enabled_stdout()` at runtime).
pub(crate) fn render_terminal_markdown_inner(md: &str, color: bool) -> String {
    if !color {
        return md.to_string();
    }

    let mut out = String::with_capacity(md.len() * 2);
    let mut sema_block = false;
    let mut other_block = false;

    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```sema") {
            sema_block = true;
            out.push_str(&paint_rgb(colors::TERTIARY, line));
            out.push('\n');
            continue;
        }
        if trimmed.starts_with("```") {
            if sema_block {
                sema_block = false;
            } else {
                other_block = !other_block;
            }
            out.push_str(&paint_rgb(colors::TERTIARY, line));
            out.push('\n');
            continue;
        }
        if sema_block {
            out.push_str(&crate::repl::highlighter::highlight_sema_ansi(line));
            out.push('\n');
            continue;
        }
        if other_block {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push_str(&paint_rgb(colors::AMBER, rest));
            out.push('\n');
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push_str(&paint_rgb(colors::AMBER, rest));
            out.push('\n');
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push_str(&paint_rgb(colors::AMBER, rest));
            out.push('\n');
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            out.push_str("• ");
            out.push_str(&style_inline_colored(rest, color));
            out.push('\n');
            continue;
        }
        out.push_str(&style_inline_colored(line, color));
        out.push('\n');
    }
    out
}

fn style_inline(text: &str) -> String {
    style_inline_colored(text, colors::enabled_stdout())
}

fn style_inline_colored(text: &str, color: bool) -> String {
    let mut out = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            let mut j = i + 2;
            while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '*') {
                j += 1;
            }
            if j + 1 < chars.len() {
                let inner: String = chars[i + 2..j].iter().collect();
                out.push_str(&maybe_rgb(color, colors::AMBER, &inner));
                i = j + 2;
                continue;
            }
        }
        if chars[i] == '`' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '`' {
                j += 1;
            }
            if j < chars.len() {
                let inner: String = chars[i + 1..j].iter().collect();
                out.push_str(&maybe_rgb(color, colors::TEAL, &inner));
                i = j + 1;
                continue;
            }
        }
        if chars[i] == '_' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '_' {
                j += 1;
            }
            if j < chars.len() {
                let inner: String = chars[i + 1..j].iter().collect();
                out.push_str(&maybe_rgb(color, colors::TERTIARY, &inner));
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub(crate) fn first_doc_line(doc: &str) -> String {
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

pub(crate) fn visible_padding(s: &str) -> usize {
    let coloured = cyan(s);
    coloured.len().saturating_sub(s.len())
}

/// Unconditional truecolor paint — the caller has already decided color is on.
fn paint_rgb(rgb: (u8, u8, u8), s: &str) -> String {
    format!("\x1b[38;2;{};{};{}m{s}\x1b[0m", rgb.0, rgb.1, rgb.2)
}

/// Paint only when `color` is true, otherwise return the text unchanged.
fn maybe_rgb(color: bool, rgb: (u8, u8, u8), s: &str) -> String {
    if color {
        paint_rgb(rgb, s)
    } else {
        s.to_string()
    }
}

fn stdout_rgb(rgb: (u8, u8, u8), s: &str) -> String {
    maybe_rgb(colors::enabled_stdout(), rgb, s)
}

fn cyan(s: &str) -> String {
    stdout_rgb(colors::TEAL, s)
}

fn dim(s: &str) -> String {
    stdout_rgb(colors::TERTIARY, s)
}

pub(crate) fn doc_name_summaries(
    extra: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    let mut summaries: BTreeMap<String, String> = BTreeMap::new();
    for entry in &sema_docs::builtin_index().entries {
        let summary = first_doc_line(&entry.summary);
        summaries
            .entry(entry.name.clone())
            .or_insert_with(|| summary.clone());
        for alias in &entry.aliases {
            summaries
                .entry(alias.clone())
                .or_insert_with(|| summary.clone());
        }
    }
    for (name, summary) in extra {
        summaries.entry(name).or_insert(summary);
    }
    summaries
}

pub(crate) fn dedupe_names(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut set = HashSet::new();
    let mut out = Vec::new();
    for name in names {
        if set.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_candidates_include_aliases() {
        let out = completion_candidates("string");
        assert!(out.contains(&"string-split".to_string()));
        assert!(out.contains(&"string/split".to_string()));
    }

    #[test]
    fn lookup_resolves_alias() {
        let entry = lookup("string-split").expect("alias entry");
        assert_eq!(entry.name, "string/split");
    }

    #[test]
    fn inline_markdown_styles_known_markers() {
        let out = style_inline("**Bold** `code` _since_");
        assert!(out.contains("Bold"));
        assert!(out.contains("code"));
        assert!(out.contains("since"));
    }

    #[test]
    fn search_results_render_empty_state() {
        let out = render_search_results("nope", &[]);
        assert!(out.contains("no documentation matches"));
    }

    // `,apropos` and `sema doc apropos` draw special forms from the sema-docs
    // index rather than injecting `SPECIAL_FORM_NAMES`. That is only complete if
    // every special form actually has a doc entry — this guards against a new
    // special form silently disappearing from apropos/lookup.
    #[test]
    fn every_special_form_has_a_doc_entry() {
        let missing: Vec<&str> = sema_eval::SPECIAL_FORM_NAMES
            .iter()
            .copied()
            .filter(|name| lookup(name).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "special forms missing a sema-docs entry: {missing:?}"
        );
    }

    #[test]
    fn markdown_inner_dims_fences_when_forced() {
        // The styled path must not depend on stdout being a TTY.
        let out = render_terminal_markdown_inner("```sema\n(a 1)\n```\n", true);
        assert!(
            out.contains("\x1b[38;2;107;99;84m"),
            "fences not dimmed: {out:?}"
        );
        // And stays plain when color is off.
        let plain = render_terminal_markdown_inner("```sema\n(a 1)\n```\n", false);
        assert!(!plain.contains('\x1b'), "unexpected ansi: {plain:?}");
    }

    #[test]
    fn first_doc_line_skips_code_and_strips_backticks() {
        // Must skip a leading fenced code block, return the first prose line, and
        // strip inline-code backticks (this drives the apropos/search summaries).
        let doc = "```sema\n(some code)\n```\nThe `real` summary line.\nSecond line.";
        assert_eq!(first_doc_line(doc), "The real summary line.");
    }

    #[test]
    fn apropos_ranks_prefix_before_substring() {
        // Prefix hits (tier 0) must outrank substring hits (tier 1) even when the
        // substring hit sorts earlier alphabetically — proving it's tiering, not
        // just an alpha sort. "flatmap" contains "map" and sorts before "map".
        let mut summaries = BTreeMap::new();
        summaries.insert("map".to_string(), String::new());
        summaries.insert("flatmap".to_string(), String::new());
        let hits = search_name_summaries("map", &summaries);
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        let prefix_pos = names.iter().position(|n| *n == "map").expect("map present");
        let substr_pos = names
            .iter()
            .position(|n| *n == "flatmap")
            .expect("flatmap present");
        assert!(
            prefix_pos < substr_pos,
            "prefix match must rank before substring match: {names:?}"
        );
    }

    #[test]
    fn apropos_hits_render_count_and_plurality() {
        let one = vec![AproposHit {
            name: "map".into(),
            summary: "apply a function".into(),
        }];
        let out = render_apropos_hits("map", &one);
        assert!(out.contains("map"), "name missing: {out:?}");
        assert!(out.contains("(1 match)"), "singular footer wrong: {out:?}");

        let two = vec![
            AproposHit {
                name: "map".into(),
                summary: "a".into(),
            },
            AproposHit {
                name: "mapcat".into(),
                summary: "b".into(),
            },
        ];
        let out2 = render_apropos_hits("map", &two);
        assert!(
            out2.contains("(2 matches)"),
            "plural footer wrong: {out2:?}"
        );
    }

    #[test]
    fn search_results_render_name_module_and_count() {
        let hits = vec![sema_mcp::docs_search::SearchHit {
            name: "string/split".into(),
            module: "strings".into(),
            signature: String::new(),
            summary: "Split a string by a delimiter".into(),
            score: 1.0,
        }];
        let out = render_search_results("split", &hits);
        assert!(out.contains("string/split"), "name missing: {out:?}");
        assert!(out.contains("[strings]"), "module tag missing: {out:?}");
        assert!(
            out.contains("Split a string by a delimiter"),
            "summary missing: {out:?}"
        );
        assert!(out.contains("(1 match)"), "count footer wrong: {out:?}");
    }

    #[test]
    fn rendered_doc_entry_shows_alias_arrow_and_kind() {
        let entry = lookup("string-split").expect("alias entry");
        let out = rendered_doc_entry("string-split", entry);
        // Alias heading resolves to the canonical name via an arrow.
        assert!(out.contains("string-split"), "alias name missing: {out:?}");
        assert!(out.contains('→'), "alias arrow missing: {out:?}");
        assert!(
            out.contains("string/split"),
            "canonical name missing: {out:?}"
        );
        assert!(out.contains("builtin"), "kind label missing: {out:?}");
    }

    #[test]
    fn rendered_doc_entry_separates_heading_from_body_with_blank_line() {
        let entry = lookup("string-split").expect("entry");
        let out = rendered_doc_entry("string-split", entry);
        let heading_end = out.find('\n').expect("heading newline");
        // A blank line must sit between the heading and the body, and there
        // must be exactly one (no double gap).
        assert!(
            out[heading_end..].starts_with("\n\n"),
            "expected blank line after heading: {out:?}"
        );
        assert!(
            !out[heading_end..].starts_with("\n\n\n"),
            "expected exactly one blank line, not a double gap: {out:?}"
        );
    }

    #[test]
    fn rendered_doc_entry_labels_special_form() {
        let entry = lookup("if").expect("special form entry");
        let out = rendered_doc_entry("if", entry);
        assert!(
            out.contains("special form"),
            "special-form label missing: {out:?}"
        );
    }
}
