use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use sema_core::{check_arity, SemaError, Value, ValueView};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::register_fn;

/// Terminal display width of `s`: ANSI escapes count as 0, wide chars as 2,
/// combining marks as 0. Shared by `string/width` and `string/word-wrap`.
fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(crate::strip_ansi(s).as_str())
}

/// Hard-break a single word (no spaces) into chunks each ≤ `width` display
/// columns, splitting on grapheme-cluster boundaries so combining sequences and
/// emoji clusters are never split mid-cluster.
fn grapheme_chunks(word: &str, width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for g in word.graphemes(true) {
        let gw = UnicodeWidthStr::width(g);
        if cur_w + gw > width && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push_str(g);
        cur_w += gw;
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// Word-wrap one paragraph (no embedded newlines) to `width` display columns.
fn wrap_paragraph(para: &str, width: usize) -> Vec<String> {
    if para.trim().is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    // Start a fresh current line from `word`, hard-breaking it if it's too wide.
    let place = |word: &str, lines: &mut Vec<String>| -> (String, usize) {
        let ww = display_width(word);
        if ww <= width {
            (word.to_string(), ww)
        } else {
            let mut chunks = grapheme_chunks(word, width);
            let last = chunks.pop().unwrap_or_default();
            lines.extend(chunks);
            let lw = display_width(&last);
            (last, lw)
        }
    };
    for word in para.split(' ') {
        if word.is_empty() {
            continue; // collapse runs of spaces
        }
        let ww = display_width(word);
        if cur.is_empty() {
            let (c, w) = place(word, &mut lines);
            cur = c;
            cur_w = w;
        } else if cur_w + 1 + ww <= width {
            cur.push(' ');
            cur.push_str(word);
            cur_w += 1 + ww;
        } else {
            lines.push(std::mem::take(&mut cur));
            let (c, w) = place(word, &mut lines);
            cur = c;
            cur_w = w;
        }
    }
    lines.push(cur);
    lines
}

thread_local! {
    static STRING_INTERN_TABLE: RefCell<HashMap<String, Rc<String>>> = RefCell::new(HashMap::new());
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "string-append", |args| {
        use std::fmt::Write;
        let mut result = String::new();
        for arg in args {
            if let Some(s) = arg.as_str() {
                result.push_str(s);
            } else {
                write!(&mut result, "{}", arg).unwrap();
            }
        }
        Ok(Value::string(&result))
    });

    register_fn(env, "string-length", |args| {
        check_arity!(args, "string-length", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::int(s.chars().count() as i64))
    });

    register_fn(env, "string-ref", |args| {
        check_arity!(args, "string-ref", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let idx_signed = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        if idx_signed < 0 {
            return Err(SemaError::eval(format!(
                "string-ref: index {idx_signed} must be non-negative"
            )));
        }
        let idx = idx_signed as usize;
        let len = s.chars().count();
        s.chars().nth(idx).map(Value::char).ok_or_else(|| {
            SemaError::eval(format!(
                "string-ref: index {idx} out of bounds (string length {len})"
            ))
            .with_hint("indices are 0-based")
        })
    });

    register_fn(env, "substring", |args| {
        check_arity!(args, "substring", 2..=3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let start_signed = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        if start_signed < 0 {
            return Err(SemaError::eval(format!(
                "substring: start index {start_signed} must be non-negative"
            )));
        }
        let start = start_signed as usize;
        let char_count = s.chars().count();
        let end = if args.len() == 3 {
            let end_signed = args[2]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?;
            if end_signed < 0 {
                return Err(SemaError::eval(format!(
                    "substring: end index {end_signed} must be non-negative"
                )));
            }
            end_signed as usize
        } else {
            char_count
        };
        if start > char_count || end > char_count || start > end {
            return Err(SemaError::eval("substring: index out of bounds"));
        }
        let start_byte = s
            .char_indices()
            .nth(start)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let end_byte = if end == char_count {
            s.len()
        } else {
            s.char_indices().nth(end).map(|(i, _)| i).unwrap_or(s.len())
        };
        Ok(Value::string(&s[start_byte..end_byte]))
    });

    register_fn(env, "string/split", |args| {
        check_arity!(args, "string/split", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sep = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let parts: Vec<Value> = s.split(sep).map(Value::string).collect();
        Ok(Value::list(parts))
    });

    // Split into lines on `\n` / `\r\n` (Clojure split-lines semantics); no trailing
    // empty line from a final newline. Use `string/split` when you need a literal sep.
    register_fn(env, "string/lines", |args| {
        check_arity!(args, "string/lines", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::list(s.lines().map(Value::string).collect()))
    });

    register_fn(env, "string/trim", |args| {
        check_arity!(args, "string/trim", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(s.trim()))
    });

    register_fn(env, "string/contains?", |args| {
        check_arity!(args, "string/contains?", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sub = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        Ok(Value::bool(s.contains(sub)))
    });

    register_fn(env, "string/starts-with?", |args| {
        check_arity!(args, "string/starts-with?", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let prefix = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        Ok(Value::bool(s.starts_with(prefix)))
    });

    register_fn(env, "string/ends-with?", |args| {
        check_arity!(args, "string/ends-with?", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let suffix = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        Ok(Value::bool(s.ends_with(suffix)))
    });

    register_fn(env, "string/upper", |args| {
        check_arity!(args, "string/upper", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&s.to_uppercase()))
    });

    register_fn(env, "string/lower", |args| {
        check_arity!(args, "string/lower", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&s.to_lowercase()))
    });

    register_fn(env, "string/replace", |args| {
        check_arity!(args, "string/replace", 3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let from = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let to = args[2]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
        Ok(Value::string(&s.replace(from, to)))
    });

    register_fn(env, "string/join", |args| {
        check_arity!(args, "string/join", 2);
        let sep = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let items = match args[0].view() {
            ValueView::List(l) => l,
            ValueView::Vector(v) => v,
            _ => return Err(SemaError::type_error("list or vector", args[0].type_name())),
        };
        let strs: Vec<String> = items
            .iter()
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.to_string()
                }
            })
            .collect();
        Ok(Value::string(&strs.join(sep)))
    });

    register_fn(env, "format", |args| {
        check_arity!(args, "format", 1..);
        let fmt = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut result = String::new();
        let mut arg_idx = 1;
        let mut chars = fmt.chars();
        while let Some(ch) = chars.next() {
            if ch == '~' {
                match chars.next() {
                    Some('a') | Some('A') => {
                        // ~a: display (no quotes)
                        if arg_idx < args.len() {
                            if let Some(s) = args[arg_idx].as_str() {
                                result.push_str(s);
                            } else {
                                result.push_str(&args[arg_idx].to_string());
                            }
                            arg_idx += 1;
                        }
                    }
                    Some('s') | Some('S') => {
                        // ~s: write (with quotes)
                        if arg_idx < args.len() {
                            result.push_str(&args[arg_idx].to_string());
                            arg_idx += 1;
                        }
                    }
                    Some('%') => result.push('\n'),
                    Some('~') => result.push('~'),
                    Some(other) => {
                        result.push('~');
                        result.push(other);
                    }
                    None => result.push('~'),
                }
            } else {
                result.push(ch);
            }
        }
        Ok(Value::string(&result))
    });

    register_fn(env, "string->symbol", |args| {
        check_arity!(args, "string->symbol", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::symbol(s))
    });

    register_fn(env, "symbol->string", |args| {
        check_arity!(args, "symbol->string", 1);
        let s = args[0]
            .as_symbol()
            .ok_or_else(|| SemaError::type_error("symbol", args[0].type_name()))?;
        Ok(Value::string(&s))
    });

    register_fn(env, "string->keyword", |args| {
        check_arity!(args, "string->keyword", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::keyword(s))
    });

    register_fn(env, "keyword->string", |args| {
        check_arity!(args, "keyword->string", 1);
        let kw = args[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
        Ok(Value::string(&kw))
    });

    register_fn(env, "number->string", |args| {
        check_arity!(args, "number->string", 1);
        match args[0].view() {
            ValueView::Int(n) => Ok(Value::string(&n.to_string())),
            ValueView::Float(f) => Ok(Value::string(&f.to_string())),
            _ => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "string->number", |args| {
        check_arity!(args, "string->number", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if let Ok(n) = s.parse::<i64>() {
            Ok(Value::int(n))
        } else if let Ok(f) = s.parse::<f64>() {
            Ok(Value::float(f))
        } else {
            Err(SemaError::eval(format!("cannot parse '{s}' as number")))
        }
    });

    register_fn(env, "string->float", |args| {
        check_arity!(args, "string->float", 1);
        match args[0].view() {
            ValueView::String(s) => s
                .parse::<f64>()
                .map(Value::float)
                .map_err(|_| SemaError::eval(format!("cannot parse '{s}' as float"))),
            ValueView::Int(n) => Ok(Value::float(n as f64)),
            ValueView::Float(_) => Ok(args[0].clone()),
            _ => Err(SemaError::type_error(
                "string or number",
                args[0].type_name(),
            )),
        }
    });

    register_fn(env, "string/index-of", |args| {
        check_arity!(args, "string/index-of", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sub = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.find(sub) {
            Some(byte_idx) => {
                let char_idx = s[..byte_idx].chars().count();
                Ok(Value::int(char_idx as i64))
            }
            None => Ok(Value::nil()),
        }
    });

    register_fn(env, "string/chars", |args| {
        check_arity!(args, "string/chars", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let chars: Vec<Value> = s.chars().map(Value::char).collect();
        Ok(Value::list(chars))
    });

    register_fn(env, "string/repeat", |args| {
        check_arity!(args, "string/repeat", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let n = args[1].as_index("string/repeat")?;
        if s.len().checked_mul(n).is_none() {
            return Err(
                SemaError::eval("string/repeat: result length overflows usize").with_hint(format!(
                    "input length {} * count {} exceeds addressable memory",
                    s.len(),
                    n
                )),
            );
        }
        Ok(Value::string(&s.repeat(n)))
    });

    register_fn(env, "string/trim-left", |args| {
        check_arity!(args, "string/trim-left", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(s.trim_start()))
    });

    register_fn(env, "string/trim-right", |args| {
        check_arity!(args, "string/trim-right", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(s.trim_end()))
    });

    register_fn(env, "string/number?", |args| {
        check_arity!(args, "string/number?", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let is_num = s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok();
        Ok(Value::bool(is_num))
    });

    register_fn(env, "string/pad-left", |args| {
        check_arity!(args, "string/pad-left", 2..=3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let width = args[1].as_index("string/pad")?;
        let pad_char = if args.len() == 3 {
            let p = args[2]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
            p.chars().next().unwrap_or(' ')
        } else {
            ' '
        };
        let char_len = s.chars().count();
        if char_len >= width {
            Ok(Value::string(s))
        } else {
            let padding: String = std::iter::repeat_n(pad_char, width - char_len).collect();
            Ok(Value::string(&format!("{}{}", padding, s)))
        }
    });

    register_fn(env, "string/pad-right", |args| {
        check_arity!(args, "string/pad-right", 2..=3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let width = args[1].as_index("string/pad")?;
        let pad_char = if args.len() == 3 {
            let p = args[2]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
            p.chars().next().unwrap_or(' ')
        } else {
            ' '
        };
        let char_len = s.chars().count();
        if char_len >= width {
            Ok(Value::string(s))
        } else {
            let padding: String = std::iter::repeat_n(pad_char, width - char_len).collect();
            Ok(Value::string(&format!("{}{}", s, padding)))
        }
    });

    register_fn(env, "string/last-index-of", |args| {
        check_arity!(args, "string/last-index-of", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sub = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.rfind(sub) {
            Some(byte_idx) => {
                let char_idx = s[..byte_idx].chars().count();
                Ok(Value::int(char_idx as i64))
            }
            None => Ok(Value::nil()),
        }
    });

    register_fn(env, "string/reverse", |args| {
        check_arity!(args, "string/reverse", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::string(&s.chars().rev().collect::<String>()))
    });

    register_fn(env, "string/empty?", |args| {
        check_arity!(args, "string/empty?", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::bool(s.is_empty()))
    });

    register_fn(env, "string/capitalize", |args| {
        check_arity!(args, "string/capitalize", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut chars = s.chars();
        let result = match chars.next() {
            Some(first) => {
                let mut r = first.to_uppercase().to_string();
                for c in chars {
                    r.extend(c.to_lowercase());
                }
                r
            }
            None => String::new(),
        };
        Ok(Value::string(&result))
    });

    register_fn(env, "string/title-case", |args| {
        check_arity!(args, "string/title-case", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let result: Vec<String> = s
            .split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => {
                        let mut w = first.to_uppercase().to_string();
                        for c in chars {
                            w.extend(c.to_lowercase());
                        }
                        w
                    }
                    None => String::new(),
                }
            })
            .collect();
        Ok(Value::string(&result.join(" ")))
    });

    // Character functions

    register_fn(env, "char->integer", |args| {
        check_arity!(args, "char->integer", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::int(c as i64))
    });

    register_fn(env, "integer->char", |args| {
        check_arity!(args, "integer->char", 1);
        let n = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        let c = char::from_u32(n as u32)
            .ok_or_else(|| SemaError::eval(format!("integer->char: invalid codepoint {n}")))?;
        Ok(Value::char(c))
    });

    register_fn(env, "char-alphabetic?", |args| {
        check_arity!(args, "char-alphabetic?", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::bool(c.is_alphabetic()))
    });

    register_fn(env, "char-numeric?", |args| {
        check_arity!(args, "char-numeric?", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::bool(c.is_numeric()))
    });

    register_fn(env, "char-whitespace?", |args| {
        check_arity!(args, "char-whitespace?", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::bool(c.is_whitespace()))
    });

    register_fn(env, "char-upper-case?", |args| {
        check_arity!(args, "char-upper-case?", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::bool(c.is_uppercase()))
    });

    register_fn(env, "char-lower-case?", |args| {
        check_arity!(args, "char-lower-case?", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::bool(c.is_lowercase()))
    });

    register_fn(env, "char-upcase", |args| {
        check_arity!(args, "char-upcase", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::char(c.to_uppercase().next().unwrap_or(c)))
    });

    register_fn(env, "char-downcase", |args| {
        check_arity!(args, "char-downcase", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::char(c.to_lowercase().next().unwrap_or(c)))
    });

    register_fn(env, "char->string", |args| {
        check_arity!(args, "char->string", 1);
        let c = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        Ok(Value::string(&c.to_string()))
    });

    register_fn(env, "string->char", |args| {
        check_arity!(args, "string->char", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut chars = s.chars();
        let c = chars
            .next()
            .ok_or_else(|| SemaError::eval("string->char: empty string"))?;
        if chars.next().is_some() {
            return Err(SemaError::eval(
                "string->char: string must have exactly one character",
            ));
        }
        Ok(Value::char(c))
    });

    register_fn(env, "string->list", |args| {
        check_arity!(args, "string->list", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let chars: Vec<Value> = s.chars().map(Value::char).collect();
        Ok(Value::list(chars))
    });

    fn two_chars(op: &str, args: &[Value]) -> Result<(char, char), SemaError> {
        check_arity!(args, op, 2);
        let a = args[0]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[0].type_name()))?;
        let b = args[1]
            .as_char()
            .ok_or_else(|| SemaError::type_error("char", args[1].type_name()))?;
        Ok((a, b))
    }

    register_fn(env, "char=?", |args| {
        let (a, b) = two_chars("char=?", args)?;
        Ok(Value::bool(a == b))
    });
    register_fn(env, "char<?", |args| {
        let (a, b) = two_chars("char<?", args)?;
        Ok(Value::bool(a < b))
    });
    register_fn(env, "char>?", |args| {
        let (a, b) = two_chars("char>?", args)?;
        Ok(Value::bool(a > b))
    });
    register_fn(env, "char<=?", |args| {
        let (a, b) = two_chars("char<=?", args)?;
        Ok(Value::bool(a <= b))
    });
    register_fn(env, "char>=?", |args| {
        let (a, b) = two_chars("char>=?", args)?;
        Ok(Value::bool(a >= b))
    });

    fn two_chars_ci(op: &str, args: &[Value]) -> Result<(char, char), SemaError> {
        let (a, b) = two_chars(op, args)?;
        let a = a.to_lowercase().next().unwrap_or(a);
        let b = b.to_lowercase().next().unwrap_or(b);
        Ok((a, b))
    }

    register_fn(env, "char-ci=?", |args| {
        let (a, b) = two_chars_ci("char-ci=?", args)?;
        Ok(Value::bool(a == b))
    });
    register_fn(env, "char-ci<?", |args| {
        let (a, b) = two_chars_ci("char-ci<?", args)?;
        Ok(Value::bool(a < b))
    });
    register_fn(env, "char-ci>?", |args| {
        let (a, b) = two_chars_ci("char-ci>?", args)?;
        Ok(Value::bool(a > b))
    });
    register_fn(env, "char-ci<=?", |args| {
        let (a, b) = two_chars_ci("char-ci<=?", args)?;
        Ok(Value::bool(a <= b))
    });
    register_fn(env, "char-ci>=?", |args| {
        let (a, b) = two_chars_ci("char-ci>=?", args)?;
        Ok(Value::bool(a >= b))
    });

    register_fn(env, "list->string", |args| {
        check_arity!(args, "list->string", 1);
        let items = args[0]
            .as_list()
            .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
        let mut s = String::with_capacity(items.len());
        for item in items {
            let c = item
                .as_char()
                .ok_or_else(|| SemaError::type_error("char", item.type_name()))?;
            s.push(c);
        }
        Ok(Value::string(&s))
    });

    register_fn(env, "string/map", |args| {
        check_arity!(args, "string/map", 2);
        let s = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let mut result = String::with_capacity(s.len());
        for ch in s.chars() {
            let mapped = crate::list::call_function(&args[0], &[Value::char(ch)])?;
            if let Some(c) = mapped.as_char() {
                result.push(c);
            } else if let Some(s) = mapped.as_str() {
                result.push_str(s);
            } else {
                return Err(SemaError::type_error("char or string", mapped.type_name()));
            }
        }
        Ok(Value::string(&result))
    });

    register_fn(env, "string/byte-length", |args| {
        check_arity!(args, "string/byte-length", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::int(s.len() as i64))
    });

    register_fn(env, "string/codepoints", |args| {
        check_arity!(args, "string/codepoints", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let codepoints: Vec<Value> = s.chars().map(|c| Value::int(c as u32 as i64)).collect();
        Ok(Value::list(codepoints))
    });

    register_fn(env, "string/from-codepoints", |args| {
        check_arity!(args, "string/from-codepoints", 1);
        let items = match args[0].view() {
            ValueView::List(l) => l,
            ValueView::Vector(v) => v,
            _ => return Err(SemaError::type_error("list or vector", args[0].type_name())),
        };
        let mut s = String::with_capacity(items.len());
        for item in items.iter() {
            let n = item
                .as_int()
                .ok_or_else(|| SemaError::type_error("integer", item.type_name()))?;
            let c = char::from_u32(n as u32).ok_or_else(|| {
                SemaError::eval(format!("string/from-codepoints: invalid codepoint {n}"))
            })?;
            s.push(c);
        }
        Ok(Value::string(&s))
    });

    register_fn(env, "string/normalize", |args| {
        check_arity!(args, "string/normalize", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let form = args[1]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| args[1].as_keyword())
            .ok_or_else(|| SemaError::type_error("string or keyword", args[1].type_name()))?;
        let normalized = match form.to_lowercase().as_str() {
            "nfc" => s.nfc().collect::<String>(),
            "nfd" => s.nfd().collect::<String>(),
            "nfkc" => s.nfkc().collect::<String>(),
            "nfkd" => s.nfkd().collect::<String>(),
            _ => {
                return Err(SemaError::eval(format!(
                    "string/normalize: unknown form {:?}",
                    form
                )))
            }
        };
        Ok(Value::string(&normalized))
    });

    register_fn(env, "string/foldcase", |args| {
        check_arity!(args, "string/foldcase", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // Full Unicode case folding (CaseFolding.txt C+F), NOT plain lowercasing:
        // e.g. "Straße" -> "strasse", final-sigma "ς" folds like "σ". This is what
        // makes foldcase the correct basis for caseless comparison, distinct from
        // string/lower (which leaves "ß" intact).
        Ok(Value::string(&caseless::default_case_fold_str(s)))
    });

    register_fn(env, "string-ci=?", |args| {
        check_arity!(args, "string-ci=?", 2);
        let a = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let b = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        // Caseless comparison via full case folding so "Straße" == "STRASSE".
        Ok(Value::bool(caseless::default_caseless_match_str(a, b)))
    });

    // string/after — everything after first occurrence of needle
    register_fn(env, "string/after", |args| {
        check_arity!(args, "string/after", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let needle = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.find(needle) {
            Some(idx) => Ok(Value::string(&s[idx + needle.len()..])),
            None => Ok(Value::string(s)),
        }
    });

    // string/after-last — everything after last occurrence of needle
    register_fn(env, "string/after-last", |args| {
        check_arity!(args, "string/after-last", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let needle = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.rfind(needle) {
            Some(idx) => Ok(Value::string(&s[idx + needle.len()..])),
            None => Ok(Value::string(s)),
        }
    });

    // string/before — everything before first occurrence of needle
    register_fn(env, "string/before", |args| {
        check_arity!(args, "string/before", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let needle = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.find(needle) {
            Some(idx) => Ok(Value::string(&s[..idx])),
            None => Ok(Value::string(s)),
        }
    });

    // string/before-last — everything before last occurrence of needle
    register_fn(env, "string/before-last", |args| {
        check_arity!(args, "string/before-last", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let needle = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.rfind(needle) {
            Some(idx) => Ok(Value::string(&s[..idx])),
            None => Ok(Value::string(s)),
        }
    });

    // string/between — portion between first occurrence of left and first occurrence of right after it
    register_fn(env, "string/between", |args| {
        check_arity!(args, "string/between", 3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let left = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let right = args[2]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
        match s.find(left) {
            Some(l_idx) => {
                let after_left = &s[l_idx + left.len()..];
                match after_left.find(right) {
                    Some(r_idx) => Ok(Value::string(&after_left[..r_idx])),
                    None => Ok(Value::string(after_left)),
                }
            }
            None => Ok(Value::string("")),
        }
    });

    // string/chop-start — remove prefix if present
    register_fn(env, "string/chop-start", |args| {
        check_arity!(args, "string/chop-start", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let prefix = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.strip_prefix(prefix) {
            Some(rest) => Ok(Value::string(rest)),
            None => Ok(Value::string(s)),
        }
    });

    // string/chop-end — remove suffix if present
    register_fn(env, "string/chop-end", |args| {
        check_arity!(args, "string/chop-end", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let suffix = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        match s.strip_suffix(suffix) {
            Some(rest) => Ok(Value::string(rest)),
            None => Ok(Value::string(s)),
        }
    });

    // string/ensure-start — ensure string starts with prefix (add if missing)
    register_fn(env, "string/ensure-start", |args| {
        check_arity!(args, "string/ensure-start", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let prefix = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        if s.starts_with(prefix) {
            Ok(Value::string(s))
        } else {
            Ok(Value::string(&format!("{}{}", prefix, s)))
        }
    });

    // string/ensure-end — ensure string ends with suffix (add if missing)
    register_fn(env, "string/ensure-end", |args| {
        check_arity!(args, "string/ensure-end", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let suffix = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        if s.ends_with(suffix) {
            Ok(Value::string(s))
        } else {
            Ok(Value::string(&format!("{}{}", s, suffix)))
        }
    });

    // string/replace-first — replace only first occurrence
    register_fn(env, "string/replace-first", |args| {
        check_arity!(args, "string/replace-first", 3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let from = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let to = args[2]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
        match s.find(from) {
            Some(idx) => {
                let mut result = String::with_capacity(s.len());
                result.push_str(&s[..idx]);
                result.push_str(to);
                result.push_str(&s[idx + from.len()..]);
                Ok(Value::string(&result))
            }
            None => Ok(Value::string(s)),
        }
    });

    // string/replace-last — replace only last occurrence
    register_fn(env, "string/replace-last", |args| {
        check_arity!(args, "string/replace-last", 3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let from = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let to = args[2]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?;
        match s.rfind(from) {
            Some(idx) => {
                let mut result = String::with_capacity(s.len());
                result.push_str(&s[..idx]);
                result.push_str(to);
                result.push_str(&s[idx + from.len()..]);
                Ok(Value::string(&result))
            }
            None => Ok(Value::string(s)),
        }
    });

    // string/remove — remove all occurrences of substring
    register_fn(env, "string/remove", |args| {
        check_arity!(args, "string/remove", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let needle = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        Ok(Value::string(&s.replace(needle, "")))
    });

    // string/take — first N chars (positive) or last N chars (negative)
    register_fn(env, "string/take", |args| {
        check_arity!(args, "string/take", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let n = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        let char_count = s.chars().count() as i64;
        if n >= 0 {
            let take = (n as usize).min(char_count as usize);
            Ok(Value::string(&s.chars().take(take).collect::<String>()))
        } else {
            let take = ((-n) as usize).min(char_count as usize);
            let skip = char_count as usize - take;
            Ok(Value::string(&s.chars().skip(skip).collect::<String>()))
        }
    });

    fn split_identifier_words(s: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut current = String::new();
        let mut prev_was_upper = false;
        let mut prev_was_sep = true;
        for ch in s.chars() {
            if ch == '_' || ch == '-' || ch == ' ' || ch == '.' {
                if !current.is_empty() {
                    words.push(current.clone());
                    current.clear();
                }
                prev_was_upper = false;
                prev_was_sep = true;
            } else if ch.is_uppercase() {
                if !current.is_empty() && (!prev_was_upper || prev_was_sep) {
                    words.push(current.clone());
                    current.clear();
                } else if !current.is_empty() && prev_was_upper && current.len() > 1 {
                    // Handle acronyms like "HTMLParser" -> ["HTML", "Parser"]
                    // We need to peek ahead — but since we don't have peek here,
                    // we'll handle it simply: consecutive uppercase stays together
                    // until a lowercase follows
                }
                current.push(ch);
                prev_was_upper = true;
                prev_was_sep = false;
            } else if ch.is_lowercase() && prev_was_upper && current.chars().count() > 1 {
                // Transition from uppercase run to lowercase: split before last uppercase
                let last = current.pop().unwrap();
                if !current.is_empty() {
                    words.push(current.clone());
                    current.clear();
                }
                current.push(last);
                current.push(ch);
                prev_was_upper = false;
                prev_was_sep = false;
            } else {
                current.push(ch);
                prev_was_upper = false;
                prev_was_sep = false;
            }
        }
        if !current.is_empty() {
            words.push(current);
        }
        words
    }

    register_fn(env, "string/snake-case", |args| {
        check_arity!(args, "string/snake-case", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let words = split_identifier_words(s);
        let result: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        Ok(Value::string(&result.join("_")))
    });

    register_fn(env, "string/kebab-case", |args| {
        check_arity!(args, "string/kebab-case", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let words = split_identifier_words(s);
        let result: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        Ok(Value::string(&result.join("-")))
    });

    register_fn(env, "string/camel-case", |args| {
        check_arity!(args, "string/camel-case", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let words = split_identifier_words(s);
        let mut result = String::new();
        for (i, word) in words.iter().enumerate() {
            if i == 0 {
                result.push_str(&word.to_lowercase());
            } else {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    result.extend(first.to_uppercase());
                    result.push_str(&chars.collect::<String>().to_lowercase());
                }
            }
        }
        Ok(Value::string(&result))
    });

    register_fn(env, "string/pascal-case", |args| {
        check_arity!(args, "string/pascal-case", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let words = split_identifier_words(s);
        let mut result = String::new();
        for word in &words {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                result.extend(first.to_uppercase());
                result.push_str(&chars.collect::<String>().to_lowercase());
            }
        }
        Ok(Value::string(&result))
    });

    register_fn(env, "string/headline", |args| {
        check_arity!(args, "string/headline", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let words = split_identifier_words(s);
        let result: Vec<String> = words
            .iter()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => {
                        let mut w = first.to_uppercase().to_string();
                        w.push_str(&chars.collect::<String>().to_lowercase());
                        w
                    }
                    None => String::new(),
                }
            })
            .collect();
        Ok(Value::string(&result.join(" ")))
    });

    register_fn(env, "string/words", |args| {
        check_arity!(args, "string/words", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let words = split_identifier_words(s);
        Ok(Value::list(
            words.into_iter().map(|w| Value::string(&w)).collect(),
        ))
    });

    // string/wrap — wrap string with left and right delimiters
    register_fn(env, "string/wrap", |args| {
        check_arity!(args, "string/wrap", 2..=3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let left = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let right = if args.len() == 3 {
            args[2]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?
        } else {
            left
        };
        Ok(Value::string(&format!("{}{}{}", left, s, right)))
    });

    // string/unwrap — remove surrounding delimiters if both present
    register_fn(env, "string/unwrap", |args| {
        check_arity!(args, "string/unwrap", 2..=3);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let left = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let right = if args.len() == 3 {
            args[2]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?
        } else {
            left
        };
        if s.starts_with(left) && s.ends_with(right) && s.len() >= left.len() + right.len() {
            Ok(Value::string(&s[left.len()..s.len() - right.len()]))
        } else {
            Ok(Value::string(s))
        }
    });

    register_fn(env, "string/intern", |args| {
        check_arity!(args, "string/intern", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let interned_rc = STRING_INTERN_TABLE.with(|table| {
            let mut table = table.borrow_mut();
            if let Some(existing) = table.get(s) {
                existing.clone()
            } else {
                let rc = Rc::new(s.to_string());
                table.insert(s.to_string(), rc.clone());
                rc
            }
        });
        Ok(Value::string_from_rc(interned_rc))
    });

    // (string/width s) -> display columns S occupies in a terminal.
    // Unlike string-length (which counts Unicode scalar values), this counts
    // TERMINAL COLUMNS: CJK and other wide characters count as 2, combining
    // marks as 0, and ANSI escape sequences (colors, cursor moves) as 0. This is
    // what TUI layout, padding, and alignment need — char count is wrong there.
    register_fn(env, "string/width", |args| {
        check_arity!(args, "string/width", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::int(display_width(s) as i64))
    });

    // (string/word-wrap text width) -> list of lines, each ≤ WIDTH display
    // columns. Word-wraps on spaces (collapsing runs), hard-breaks words longer
    // than WIDTH by grapheme cluster, preserves explicit newlines as line breaks,
    // and measures with display width so wrapping is correct for non-ASCII text.
    // (Distinct from `string/wrap`, which wraps a string in delimiters.)
    register_fn(env, "string/word-wrap", |args| {
        check_arity!(args, "string/word-wrap", 2);
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let width = args[1].as_index("string/word-wrap")?.max(1);
        let mut out = Vec::new();
        for para in text.split('\n') {
            for line in wrap_paragraph(para, width) {
                out.push(Value::string(&line));
            }
        }
        Ok(Value::list(out))
    });

    // Silent aliases for other Lisp dialects (undocumented)
    if let Some(v) = env.get(sema_core::intern("string/join")) {
        env.set(sema_core::intern("string-join"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string/split")) {
        env.set(sema_core::intern("string-split"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string/trim")) {
        env.set(sema_core::intern("string-trim"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string/repeat")) {
        env.set(sema_core::intern("make-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string/upper")) {
        env.set(sema_core::intern("string-upcase"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string/lower")) {
        env.set(sema_core::intern("string-downcase"), v);
    }

    // module/function aliases for legacy Scheme names
    if let Some(v) = env.get(sema_core::intern("string-append")) {
        env.set(sema_core::intern("string/append"), v.clone());
        // `str` is Clojure-style stringify; identical to string-append today.
        env.set(sema_core::intern("str"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string-length")) {
        env.set(sema_core::intern("string/length"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string-ref")) {
        env.set(sema_core::intern("string/ref"), v);
    }
    if let Some(v) = env.get(sema_core::intern("substring")) {
        env.set(sema_core::intern("string/slice"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->symbol")) {
        env.set(sema_core::intern("string/to-symbol"), v);
    }
    if let Some(v) = env.get(sema_core::intern("symbol->string")) {
        env.set(sema_core::intern("symbol/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->keyword")) {
        env.set(sema_core::intern("string/to-keyword"), v);
    }
    if let Some(v) = env.get(sema_core::intern("keyword->string")) {
        env.set(sema_core::intern("keyword/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("number->string")) {
        env.set(sema_core::intern("number/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->number")) {
        env.set(sema_core::intern("string/to-number"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->float")) {
        env.set(sema_core::intern("string/to-float"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char->integer")) {
        env.set(sema_core::intern("char/to-integer"), v);
    }
    if let Some(v) = env.get(sema_core::intern("integer->char")) {
        env.set(sema_core::intern("integer/to-char"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char->string")) {
        env.set(sema_core::intern("char/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->char")) {
        env.set(sema_core::intern("string/to-char"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->list")) {
        env.set(sema_core::intern("string/to-list"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-alphabetic?")) {
        env.set(sema_core::intern("char/alphabetic?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-numeric?")) {
        env.set(sema_core::intern("char/numeric?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-whitespace?")) {
        env.set(sema_core::intern("char/whitespace?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-upper-case?")) {
        env.set(sema_core::intern("char/upper-case?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-lower-case?")) {
        env.set(sema_core::intern("char/lower-case?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-upcase")) {
        env.set(sema_core::intern("char/upcase"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-downcase")) {
        env.set(sema_core::intern("char/downcase"), v);
    }
}
