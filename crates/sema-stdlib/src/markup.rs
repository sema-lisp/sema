//! Markdown and HTML helper builtins.
//!
//! Markdown rendering and heading extraction use `pulldown-cmark`; HTML parsing
//! and CSS selection use `scraper` (html5ever).
//!
//! These functions are all pure (no filesystem or network access), so they need
//! no sandbox gating.
//!
//! **Bounded / offloaded CPU (B9 R21 split).** `html/parse`/`select`/`text`/
//! `select-text` build a full DOM (html5ever), which is CPU-bound on large input,
//! so during a runtime quantum (`in_runtime_quantum()`) each captures a per-input
//! byte cap BEFORE dispatch and offloads the parse+select onto the I/O pool
//! through `quarantined_compute` (`io.rs`). The parse runs over an owned `String`
//! snapshot (`Send`; the selector string too) on a worker and returns `Send`
//! strings — the normalized HTML, collected text, or per-match outer-HTML/text —
//! which are decoded back into a `Value` on the VM thread. The worker also caps
//! the parsed DOM node count as a secondary guardrail. No `Value`/`Env` crosses
//! the thread boundary. The `markdown/*` helpers are streaming O(input) passes, so
//! they stay SYNCHRONOUS with only a pre-dispatch input-byte cap inside a quantum
//! (bounded input ⇒ bounded VM-thread CPU) — an explicit synchronous split, not a
//! fake async wrap. A direct native call outside the cooperative runtime keeps the
//! uncapped synchronous shape.

use std::cell::Cell;
use std::collections::BTreeMap;

use pulldown_cmark::{Event, HeadingLevel, Tag, TagEnd};
use scraper::{Html, Selector};
use sema_core::runtime::NativeOutcome;
use sema_core::{check_arity, SemaError, Value};

use crate::{register_fn, register_runtime_fn};

/// Per-input byte cap for the `html/*` and `markdown/*` ops under a runtime
/// quantum. DOM construction is heavier per byte than a line diff, so this ceiling
/// (32 MiB) is tighter than `diff`'s — still far above any realistic document.
const MARKUP_INPUT_BYTE_CAP: u64 = 32 * 1024 * 1024;
/// Parsed-DOM node-count cap enforced on the worker after `html5ever` parses.
/// Generous relative to the byte cap; a guardrail, not the terminal bound.
const MARKUP_NODE_CAP: usize = 5_000_000;

thread_local! {
    /// Optional per-call input-byte cap override (lowered, never raised above the
    /// hard ceiling). Read on the VM thread pre-dispatch; mirrors
    /// `diff::DIFF_INPUT_BYTE_CAP_OVERRIDE`. `None` uses the module ceiling. The
    /// seam the regression suite drives to exercise the cap boundary without a
    /// multi-megabyte input string.
    static MARKUP_INPUT_BYTE_CAP_OVERRIDE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The effective per-input byte cap for the current call: the module ceiling,
/// lowered by any per-call override (never raised above it).
fn effective_markup_input_byte_cap() -> u64 {
    MARKUP_INPUT_BYTE_CAP_OVERRIDE
        .with(Cell::get)
        .map_or(MARKUP_INPUT_BYTE_CAP, |over| {
            over.min(MARKUP_INPUT_BYTE_CAP)
        })
}

/// Lower the per-input byte cap (clamped to the hard ceiling) for subsequent
/// markup calls on this thread, or clear the override with `None`. Test seam,
/// mirroring `set_diff_input_byte_cap_override`.
pub fn set_markup_input_byte_cap_override(bytes: Option<u64>) {
    MARKUP_INPUT_BYTE_CAP_OVERRIDE.with(|cell| cell.set(bytes));
}

/// Reject `actual` bytes over `limit`. Reads the argument's existing `len()` — no
/// snapshot — so an over-cap input is rejected without any excess allocation.
fn check_markup_limit(op: &str, actual: u64, limit: u64) -> Result<(), SemaError> {
    if actual > limit {
        return Err(SemaError::eval(format!(
            "{op}: input bytes {actual} exceeds the quarantined limit {limit}"
        ))
        .with_hint("reduce or split the markup input"));
    }
    Ok(())
}

/// Reject a parsed DOM whose node count exceeds the guardrail (runs on the worker
/// after `html5ever` parses the byte-capped snapshot).
fn check_markup_nodes(op: &str, doc: &Html) -> Result<(), SemaError> {
    let nodes = doc.tree.values().count();
    if nodes > MARKUP_NODE_CAP {
        return Err(SemaError::eval(format!(
            "{op}: DOM nodes {nodes} exceeds the quarantined limit {MARKUP_NODE_CAP}"
        ))
        .with_hint("reduce or split the markup input"));
    }
    Ok(())
}

/// `html/parse` work: parse, node-cap, and return the normalized round-trip HTML.
fn html_parse_work(html: &str) -> Result<String, SemaError> {
    let doc = Html::parse_document(html);
    check_markup_nodes("html/parse", &doc)?;
    Ok(doc.html())
}

/// `html/select` work: outer HTML of every element matching `sel`.
fn html_select_work(html: &str, sel: &str) -> Result<Vec<String>, SemaError> {
    let selector =
        Selector::parse(sel).map_err(|e| SemaError::eval(format!("invalid selector: {e:?}")))?;
    let doc = Html::parse_document(html);
    check_markup_nodes("html/select", &doc)?;
    Ok(doc.select(&selector).map(|el| el.html()).collect())
}

/// `html/text` work: concatenated, whitespace-collapsed visible text.
fn html_text_work(html: &str) -> Result<String, SemaError> {
    let doc = Html::parse_document(html);
    check_markup_nodes("html/text", &doc)?;
    let text = doc.root_element().text().collect::<Vec<_>>().join(" ");
    Ok(collapse_whitespace(&text))
}

/// `html/select-text` work: text of every element matching `sel`.
fn html_select_text_work(html: &str, sel: &str) -> Result<Vec<String>, SemaError> {
    let selector =
        Selector::parse(sel).map_err(|e| SemaError::eval(format!("invalid selector: {e:?}")))?;
    let doc = Html::parse_document(html);
    check_markup_nodes("html/select-text", &doc)?;
    Ok(doc
        .select(&selector)
        .map(|el| {
            let text = el.text().collect::<Vec<_>>().join(" ");
            collapse_whitespace(&text)
        })
        .collect())
}

/// Build a list-of-strings `Value`. A plain `fn` (no captures) so it fits
/// `quarantined_compute`'s `fn(T) -> Value` decoder slot.
fn html_strings_to_value(items: Vec<String>) -> Value {
    Value::list(items.iter().map(|s| Value::string(s)).collect())
}

fn heading_level_to_int(level: HeadingLevel) -> i64 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Collapse runs of whitespace into single spaces and trim the ends.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn register(env: &sema_core::Env) {
    // (markdown/to-html md) -> CommonMark rendered to an HTML string. A streaming
    // O(input) pass, so it stays synchronous; inside a runtime quantum a
    // pre-dispatch input-byte cap bounds its VM-thread CPU.
    register_fn(env, "markdown/to-html", |args| {
        check_arity!(args, "markdown/to-html", 1);
        let md = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_markup_limit(
                "markdown/to-html",
                md.len() as u64,
                effective_markup_input_byte_cap(),
            )?;
        }
        let parser = pulldown_cmark::Parser::new(md);
        let mut out = String::new();
        pulldown_cmark::html::push_html(&mut out, parser);
        Ok(Value::string(&out))
    });

    // (markdown/headings md) -> list of {:level <int> :text "..."} in doc order.
    register_fn(env, "markdown/headings", |args| {
        check_arity!(args, "markdown/headings", 1);
        let md = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_markup_limit(
                "markdown/headings",
                md.len() as u64,
                effective_markup_input_byte_cap(),
            )?;
        }
        let parser = pulldown_cmark::Parser::new(md);

        let mut headings: Vec<Value> = Vec::new();
        let mut current_level: Option<i64> = None;
        let mut current_text = String::new();

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    current_level = Some(heading_level_to_int(level));
                    current_text.clear();
                }
                Event::Text(t) | Event::Code(t) if current_level.is_some() => {
                    current_text.push_str(&t);
                }
                // A soft/hard break inside a heading is a word boundary; emit a
                // space so `# line one\n  line two` doesn't become "line oneline two".
                Event::SoftBreak | Event::HardBreak if current_level.is_some() => {
                    current_text.push(' ');
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some(level) = current_level.take() {
                        let mut m: BTreeMap<Value, Value> = BTreeMap::new();
                        m.insert(Value::keyword("level"), Value::int(level));
                        m.insert(
                            Value::keyword("text"),
                            Value::string(&collapse_whitespace(&current_text)),
                        );
                        headings.push(Value::map(m));
                    }
                    current_text.clear();
                }
                _ => {}
            }
        }

        Ok(Value::list(headings))
    });

    // (markdown/frontmatter md) -> {:frontmatter "<raw>" :body "<rest>"} or
    // {:frontmatter nil :body "<original>"} when there is no leading `---` fence.
    register_fn(env, "markdown/frontmatter", |args| {
        check_arity!(args, "markdown/frontmatter", 1);
        let md = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_markup_limit(
                "markdown/frontmatter",
                md.len() as u64,
                effective_markup_input_byte_cap(),
            )?;
        }

        let mut m: BTreeMap<Value, Value> = BTreeMap::new();

        // A frontmatter block must start with a `---` line at the very top.
        if let Some(rest) = md
            .strip_prefix("---\n")
            .or_else(|| md.strip_prefix("---\r\n"))
        {
            // Find the closing `---` fence on its own line.
            if let Some((block, body)) = split_closing_fence(rest) {
                m.insert(Value::keyword("frontmatter"), Value::string(block));
                m.insert(Value::keyword("body"), Value::string(body));
                return Ok(Value::map(m));
            }
        }

        m.insert(Value::keyword("frontmatter"), Value::nil());
        m.insert(Value::keyword("body"), Value::string(md));
        Ok(Value::map(m))
    });

    // (html/parse html) -> validates/normalizes and returns the HTML string
    // itself. The other html/* functions re-parse this string internally with
    // scraper. This avoids needing a foreign-handle Value type. DOM construction
    // is CPU-bound, so in a runtime quantum it captures a per-input byte cap BEFORE
    // dispatch and offloads onto the I/O pool via `quarantined_compute`.
    register_runtime_fn(env, "html/parse", |args| {
        check_arity!(args, "html/parse", 1);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_markup_limit(
                "html/parse",
                html.len() as u64,
                effective_markup_input_byte_cap(),
            )?;
            let snapshot = html.to_string();
            return crate::io::quarantined_compute("html/parse", Value::string_owned, move || {
                html_parse_work(&snapshot).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::string_owned(html_parse_work(
            html,
        )?)))
    });

    // (html/select html selector) -> list of strings, each the outer HTML of a
    // matched element.
    register_runtime_fn(env, "html/select", |args| {
        check_arity!(args, "html/select", 2);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sel = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_markup_limit(
                "html/select",
                html.len() as u64,
                effective_markup_input_byte_cap(),
            )?;
            let html = html.to_string();
            let sel = sel.to_string();
            return crate::io::quarantined_compute(
                "html/select",
                html_strings_to_value,
                move || html_select_work(&html, &sel).map_err(|e| e.to_string()),
            );
        }
        Ok(NativeOutcome::Return(html_strings_to_value(
            html_select_work(html, sel)?,
        )))
    });

    // (html/text html) -> concatenated, whitespace-collapsed visible text.
    register_runtime_fn(env, "html/text", |args| {
        check_arity!(args, "html/text", 1);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_markup_limit(
                "html/text",
                html.len() as u64,
                effective_markup_input_byte_cap(),
            )?;
            let snapshot = html.to_string();
            return crate::io::quarantined_compute("html/text", Value::string_owned, move || {
                html_text_work(&snapshot).map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(Value::string_owned(html_text_work(
            html,
        )?)))
    });

    // (html/select-text html selector) -> list of strings, the text of each
    // matched element.
    register_runtime_fn(env, "html/select-text", |args| {
        check_arity!(args, "html/select-text", 2);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sel = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_markup_limit(
                "html/select-text",
                html.len() as u64,
                effective_markup_input_byte_cap(),
            )?;
            let html = html.to_string();
            let sel = sel.to_string();
            return crate::io::quarantined_compute(
                "html/select-text",
                html_strings_to_value,
                move || html_select_text_work(&html, &sel).map_err(|e| e.to_string()),
            );
        }
        Ok(NativeOutcome::Return(html_strings_to_value(
            html_select_text_work(html, sel)?,
        )))
    });
}

/// Given the text following the opening `---\n` fence, find the next line that is
/// exactly `---` (optionally followed by `\r`) and split there. Returns the raw
/// frontmatter block (text between the fences, without the fence lines) and the
/// body (everything after the closing fence's line break).
fn split_closing_fence(rest: &str) -> Option<(&str, &str)> {
    let mut line_start = 0usize;
    loop {
        // Determine the current line [line_start, line_end).
        let line_end = match rest[line_start..].find('\n') {
            Some(off) => line_start + off,
            None => rest.len(),
        };
        let line = rest[line_start..line_end]
            .strip_suffix('\r')
            .unwrap_or(&rest[line_start..line_end]);
        if line == "---" {
            let block = &rest[..line_start];
            // Body begins after this line's newline (if any).
            let body_start = if line_end < rest.len() {
                line_end + 1 // skip '\n'
            } else {
                rest.len()
            };
            return Some((block, &rest[body_start..]));
        }
        if line_end >= rest.len() {
            break;
        }
        line_start = line_end + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::Env;

    fn test_env() -> Env {
        let env = Env::new();
        register(&env);
        env
    }

    fn call(env: &Env, name: &str, args: &[Value]) -> Result<Value, SemaError> {
        let f = env
            .get(sema_core::intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        let nf = f.as_native_fn_ref().expect("native fn");
        let ctx = sema_core::EvalContext::default();
        (nf.func)(&ctx, args)
    }

    #[test]
    fn markdown_to_html_renders_h1() {
        let env = test_env();
        let out = call(&env, "markdown/to-html", &[Value::string("# Hi")]).unwrap();
        let s = out.as_str().unwrap();
        assert!(s.contains("<h1>"), "expected <h1> in {s:?}");
    }

    #[test]
    fn markdown_headings_levels_and_text() {
        let env = test_env();
        let md = "# First\n\n## Second\n";
        let out = call(&env, "markdown/headings", &[Value::string(md)]).unwrap();
        let list = out.as_list_rc().expect("list");
        assert_eq!(list.len(), 2);

        let h0 = list[0].as_map_rc().unwrap();
        assert_eq!(
            h0.get(&Value::keyword("level")).and_then(|v| v.as_int()),
            Some(1)
        );
        assert_eq!(
            h0.get(&Value::keyword("text"))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
            Some("First".to_string())
        );

        let h1 = list[1].as_map_rc().unwrap();
        assert_eq!(
            h1.get(&Value::keyword("level")).and_then(|v| v.as_int()),
            Some(2)
        );
        assert_eq!(
            h1.get(&Value::keyword("text"))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
            Some("Second".to_string())
        );
    }

    #[test]
    fn markdown_frontmatter_present_and_absent() {
        let env = test_env();

        let md = "---\ntitle: Hello\n---\nBody here\n";
        let out = call(&env, "markdown/frontmatter", &[Value::string(md)]).unwrap();
        let m = out.as_map_rc().unwrap();
        assert_eq!(
            m.get(&Value::keyword("frontmatter"))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
            Some("title: Hello\n".to_string())
        );
        assert_eq!(
            m.get(&Value::keyword("body"))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
            Some("Body here\n".to_string())
        );

        let plain = "no frontmatter here";
        let out = call(&env, "markdown/frontmatter", &[Value::string(plain)]).unwrap();
        let m = out.as_map_rc().unwrap();
        assert!(m
            .get(&Value::keyword("frontmatter"))
            .map(|v| v.is_nil())
            .unwrap_or(false));
        assert_eq!(
            m.get(&Value::keyword("body"))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
            Some(plain.to_string())
        );
    }

    #[test]
    fn html_select_filters_by_class() {
        let env = test_env();
        let html = "<p class=x>a</p><p>b</p>";
        let out = call(
            &env,
            "html/select",
            &[Value::string(html), Value::string("p.x")],
        )
        .unwrap();
        let list = out.as_list_rc().expect("list");
        assert_eq!(list.len(), 1);
        assert!(list[0].as_str().unwrap().contains(">a<"));
    }

    #[test]
    fn html_select_invalid_selector_errors() {
        let env = test_env();
        let html = "<p>a</p>";
        let err = call(
            &env,
            "html/select",
            &[Value::string(html), Value::string(">>>bad")],
        );
        assert!(err.is_err());
    }

    #[test]
    fn html_text_strips_tags() {
        let env = test_env();
        let html = "<div><p>hello</p> <p>world</p></div>";
        let out = call(&env, "html/text", &[Value::string(html)]).unwrap();
        let s = out.as_str().unwrap();
        assert!(s.contains("hello"));
        assert!(s.contains("world"));
        assert!(!s.contains('<'));
    }

    #[test]
    fn html_select_text_returns_text() {
        let env = test_env();
        let html = "<p class=x>alpha</p><p class=x>beta</p>";
        let out = call(
            &env,
            "html/select-text",
            &[Value::string(html), Value::string("p.x")],
        )
        .unwrap();
        let list = out.as_list_rc().expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].as_str().unwrap(), "alpha");
        assert_eq!(list[1].as_str().unwrap(), "beta");
    }

    #[test]
    fn markup_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_markup_limit("html/select", 8, 8).is_ok());
        let error = check_markup_limit("html/select", 9, 8)
            .expect_err("one byte over the captured limit must fail");
        assert!(error.to_string().contains('9'));
        assert!(error.to_string().contains('8'));
    }

    #[test]
    fn markup_input_byte_cap_is_finite_and_clamps_overrides() {
        assert_eq!(effective_markup_input_byte_cap(), MARKUP_INPUT_BYTE_CAP);
        set_markup_input_byte_cap_override(Some(16));
        assert_eq!(effective_markup_input_byte_cap(), 16);
        // An override above the hard ceiling is clamped down, never raised.
        set_markup_input_byte_cap_override(Some(u64::MAX));
        assert_eq!(effective_markup_input_byte_cap(), MARKUP_INPUT_BYTE_CAP);
        set_markup_input_byte_cap_override(None);
        assert_eq!(effective_markup_input_byte_cap(), MARKUP_INPUT_BYTE_CAP);
    }

    /// A small DOM passes the node guardrail (the input-byte cap is the terminal
    /// bound; the node cap only backstops a pathological expansion).
    #[test]
    fn html_select_work_passes_node_guardrail() {
        let rows = html_select_work("<p class=x>a</p><p>b</p>", "p.x").unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains(">a<"));
    }
}
