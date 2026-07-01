//! Markdown and HTML helper builtins.
//!
//! Markdown rendering and heading extraction use `pulldown-cmark`; HTML parsing
//! and CSS selection use `scraper` (html5ever).
//!
//! These functions are all pure (no filesystem or network access), so they need
//! no sandbox gating.

use std::collections::BTreeMap;

use pulldown_cmark::{Event, HeadingLevel, Tag, TagEnd};
use scraper::{Html, Selector};
use sema_core::{check_arity, SemaError, Value};

use crate::register_fn;

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
    // (markdown/to-html md) -> CommonMark rendered to an HTML string.
    register_fn(env, "markdown/to-html", |args| {
        check_arity!(args, "markdown/to-html", 1);
        let md = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
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
    // scraper. This avoids needing a foreign-handle Value type.
    register_fn(env, "html/parse", |args| {
        check_arity!(args, "html/parse", 1);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Parsing always succeeds (html5ever is lenient); this validates that the
        // input is a string and yields a normalized round-trip of the document.
        let doc = Html::parse_document(html);
        Ok(Value::string(&doc.html()))
    });

    // (html/select html selector) -> list of strings, each the outer HTML of a
    // matched element.
    register_fn(env, "html/select", |args| {
        check_arity!(args, "html/select", 2);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sel = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let selector = Selector::parse(sel)
            .map_err(|e| SemaError::eval(format!("invalid selector: {e:?}")))?;
        let doc = Html::parse_document(html);
        let results: Vec<Value> = doc
            .select(&selector)
            .map(|el| Value::string(&el.html()))
            .collect();
        Ok(Value::list(results))
    });

    // (html/text html) -> concatenated, whitespace-collapsed visible text.
    register_fn(env, "html/text", |args| {
        check_arity!(args, "html/text", 1);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let doc = Html::parse_document(html);
        let text = doc.root_element().text().collect::<Vec<_>>().join(" ");
        Ok(Value::string(&collapse_whitespace(&text)))
    });

    // (html/select-text html selector) -> list of strings, the text of each
    // matched element.
    register_fn(env, "html/select-text", |args| {
        check_arity!(args, "html/select-text", 2);
        let html = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sel = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let selector = Selector::parse(sel)
            .map_err(|e| SemaError::eval(format!("invalid selector: {e:?}")))?;
        let doc = Html::parse_document(html);
        let results: Vec<Value> = doc
            .select(&selector)
            .map(|el| {
                let text = el.text().collect::<Vec<_>>().join(" ");
                Value::string(&collapse_whitespace(&text))
            })
            .collect();
        Ok(Value::list(results))
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
}
