//! Small UTF-8-safe string helpers shared across crates.

use crate::Value;

/// Return the prefix of `s` containing at most `max_chars` characters, always
/// landing on a UTF-8 char boundary. Replaces the `&s[..N]` byte-slicing
/// (Pattern B in the 2026-05-29 audit) that panics when byte `N` falls inside a
/// multi-byte character.
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Substitute `{{key}}` placeholders in a template from a keyword-keyed map.
/// A key with no entry is left as written (so a typo stays visible); a value
/// that is not a string is rendered with `Display`. Used by `prompt/render`
/// and `prompt/fill`.
pub fn render_template(template: &str, vars: &std::collections::BTreeMap<Value, Value>) -> String {
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
                    if let Some(s) = val.as_str() {
                        result.push_str(s);
                    } else {
                        result.push_str(&val.to_string());
                    }
                } else {
                    result.push_str("{{");
                    result.push_str(&var_name);
                    result.push_str("}}");
                }
            } else {
                result.push_str("{{");
                result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_chars_never_splits_a_char() {
        // 40 lambdas prefixed by one ASCII char: byte 39 lands inside a 2-byte 'λ'.
        let s = format!("x{}", "λ".repeat(40));
        let out = truncate_chars(&s, 39);
        // Must not panic and must be a valid prefix on a char boundary.
        assert!(s.starts_with(out));
        assert_eq!(out.chars().count(), 39);
    }

    #[test]
    fn truncate_chars_returns_whole_string_when_short() {
        assert_eq!(truncate_chars("hello", 39), "hello");
    }
}
