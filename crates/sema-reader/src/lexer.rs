use sema_core::{SemaError, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Expr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Quote,
    Quasiquote,
    Unquote,
    UnquoteSplice,
    Deref,
    Int(i64),
    Float(f64),
    String(String),
    FString(Vec<FStringPart>),
    ShortLambdaStart,
    Symbol(String),
    Keyword(String),
    Bool(bool),
    Char(char),
    BytevectorStart,
    Dot,
    Comment(String),
    Newline,
    Regex(String),
}

#[derive(Debug, Clone)]
pub struct SpannedToken {
    pub token: Token,
    pub span: Span,
    /// Byte offset of the start of this token in the source string.
    pub byte_start: usize,
    /// Byte offset past the end of this token in the source string.
    pub byte_end: usize,
}

pub fn tokenize(input: &str) -> Result<Vec<SpannedToken>, SemaError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    // Build char-index → byte-offset lookup table for string source extraction
    let byte_offsets: Vec<usize> = {
        let mut offsets = Vec::with_capacity(chars.len() + 1);
        let mut pos = 0;
        for c in &chars {
            offsets.push(pos);
            pos += c.len_utf8();
        }
        offsets.push(pos);
        offsets
    };
    let mut i = 0;
    let mut line = 1;
    let mut col = 1;

    while i < chars.len() {
        let ch = chars[i];
        let span = Span::point(line, col);

        match ch {
            // Whitespace
            ' ' | '\t' | '\r' => {
                col += 1;
                i += 1;
            }
            '\n' => {
                tokens.push(SpannedToken {
                    token: Token::Newline,
                    span: span.with_end(line, col + 1),
                    byte_start: byte_offsets[i],
                    byte_end: byte_offsets[i + 1],
                });
                line += 1;
                col = 1;
                i += 1;
            }

            // Comments
            ';' => {
                let start = i;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let end_col = col + (i - start);
                tokens.push(SpannedToken {
                    token: Token::Comment(text),
                    span: span.with_end(line, end_col),
                    byte_start: byte_offsets[start],
                    byte_end: byte_offsets[i],
                });
                col = end_col;
            }

            // Delimiters
            '(' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::LParen,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }
            ')' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::RParen,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }
            '[' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::LBracket,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }
            ']' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::RBracket,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }
            '{' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::LBrace,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }
            '}' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::RBrace,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }

            // Quote forms
            '\'' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::Quote,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }
            '`' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::Quasiquote,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }
            ',' => {
                if i + 1 < chars.len() && chars[i + 1] == '@' {
                    col += 2;
                    i += 2;
                    tokens.push(SpannedToken {
                        token: Token::UnquoteSplice,
                        span: span.with_end(line, col),
                        byte_start: byte_offsets[i - 2],
                        byte_end: byte_offsets[i],
                    });
                } else {
                    col += 1;
                    i += 1;
                    tokens.push(SpannedToken {
                        token: Token::Unquote,
                        span: span.with_end(line, col),
                        byte_start: byte_offsets[i - 1],
                        byte_end: byte_offsets[i],
                    });
                }
            }

            // Deref reader macro: @expr -> (deref expr)
            '@' => {
                col += 1;
                i += 1;
                tokens.push(SpannedToken {
                    token: Token::Deref,
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[i - 1],
                    byte_end: byte_offsets[i],
                });
            }

            // Strings
            '"' => {
                let token_start = i;
                let mut s = String::new();
                i += 1;
                col += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        col += 1;
                        read_string_escape(&chars, &mut i, &mut col, &mut s, span)?;
                    } else {
                        if chars[i] == '\n' {
                            line += 1;
                            col = 0;
                        }
                        s.push(chars[i]);
                    }
                    i += 1;
                    col += 1;
                }
                if i >= chars.len() {
                    return Err(SemaError::Reader {
                        message: "unterminated string".to_string(),
                        span,
                    });
                }
                i += 1; // closing quote
                col += 1;
                tokens.push(SpannedToken {
                    token: Token::String(s),
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[token_start],
                    byte_end: byte_offsets[i],
                });
            }

            // #t, #f booleans
            '#' => {
                let token_start = i;
                if i + 1 < chars.len() {
                    match chars[i + 1] {
                        't' => {
                            i += 2;
                            col += 2;
                            tokens.push(SpannedToken {
                                token: Token::Bool(true),
                                span: span.with_end(line, col),
                                byte_start: byte_offsets[token_start],
                                byte_end: byte_offsets[i],
                            });
                        }
                        'f' => {
                            i += 2;
                            col += 2;
                            tokens.push(SpannedToken {
                                token: Token::Bool(false),
                                span: span.with_end(line, col),
                                byte_start: byte_offsets[token_start],
                                byte_end: byte_offsets[i],
                            });
                        }
                        '\\' => {
                            // Character literal: #\a, #\space, #\newline, etc.
                            i += 2; // skip #\
                            col += 2;
                            if i >= chars.len() {
                                return Err(SemaError::Reader {
                                    message: "unexpected end of input after #\\".to_string(),
                                    span,
                                });
                            }
                            let start = i;
                            if chars[i].is_alphabetic() {
                                while i < chars.len() && is_symbol_char(chars[i]) {
                                    i += 1;
                                    col += 1;
                                }
                            } else {
                                i += 1;
                                col += 1;
                            }
                            let name: String = chars[start..i].iter().collect();
                            let c = match name.as_str() {
                                "space" => ' ',
                                "newline" => '\n',
                                "tab" => '\t',
                                "return" => '\r',
                                "nul" => '\0',
                                s if s.chars().count() == 1 => s.chars().next().unwrap(),
                                _ => {
                                    return Err(SemaError::Reader {
                                        message: format!("unknown character name: {name}"),
                                        span,
                                    });
                                }
                            };
                            tokens.push(SpannedToken {
                                token: Token::Char(c),
                                span: span.with_end(line, col),
                                byte_start: byte_offsets[token_start],
                                byte_end: byte_offsets[i],
                            });
                        }
                        'u' if i + 3 < chars.len()
                            && chars[i + 2] == '8'
                            && chars[i + 3] == '(' =>
                        {
                            i += 4;
                            col += 4;
                            tokens.push(SpannedToken {
                                token: Token::BytevectorStart,
                                span: span.with_end(line, col),
                                byte_start: byte_offsets[token_start],
                                byte_end: byte_offsets[i],
                            });
                        }
                        '(' => {
                            // Short lambda: #(+ % 1) → (lambda (%1) (+ %1 1))
                            i += 2; // skip #(
                            col += 2;
                            tokens.push(SpannedToken {
                                token: Token::ShortLambdaStart,
                                span: span.with_end(line, col),
                                byte_start: byte_offsets[token_start],
                                byte_end: byte_offsets[i],
                            });
                        }
                        '"' => {
                            // Regex literal: #"pattern" — raw string (no escape processing)
                            i += 2; // skip #"
                            col += 2;
                            let mut s = String::new();
                            while i < chars.len() && chars[i] != '"' {
                                if chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1] == '"' {
                                    s.push('"');
                                    i += 2;
                                    col += 2;
                                } else {
                                    if chars[i] == '\n' {
                                        line += 1;
                                        col = 0;
                                    }
                                    s.push(chars[i]);
                                    i += 1;
                                    col += 1;
                                }
                            }
                            if i >= chars.len() {
                                return Err(SemaError::Reader {
                                    message: "unterminated regex literal".to_string(),
                                    span,
                                }
                                .with_hint(
                                    "add a closing `\"` to end the #\"...\" regex literal",
                                ));
                            }
                            i += 1; // closing quote
                            col += 1;
                            tokens.push(SpannedToken {
                                token: Token::Regex(s),
                                span: span.with_end(line, col),
                                byte_start: byte_offsets[token_start],
                                byte_end: byte_offsets[i],
                            });
                        }
                        '!' if line == 1 && col == 1 => {
                            // Shebang line: #!/usr/bin/env sema
                            while i < chars.len() && chars[i] != '\n' {
                                i += 1;
                            }
                        }
                        _ => {
                            return Err(SemaError::Reader {
                                message: format!(
                                    "unexpected character after #: '{}'",
                                    chars[i + 1]
                                ),
                                span,
                            });
                        }
                    }
                } else {
                    return Err(SemaError::Reader {
                        message: "unexpected end of input after `#`".to_string(),
                        span,
                    }
                    .with_hint("# starts a special form: #t, #f, #\\char, #u8(...)"));
                }
            }

            // Keywords (:foo)
            ':' => {
                let token_start = i;
                i += 1;
                col += 1;
                let start = i;
                while i < chars.len() && is_symbol_char(chars[i]) {
                    i += 1;
                    col += 1;
                }
                if i == start {
                    return Err(SemaError::Reader {
                        message: "expected keyword name after ':'".to_string(),
                        span,
                    });
                }
                let name: String = chars[start..i].iter().collect();
                tokens.push(SpannedToken {
                    token: Token::Keyword(name),
                    span: span.with_end(line, col),
                    byte_start: byte_offsets[token_start],
                    byte_end: byte_offsets[i],
                });
            }

            // Numbers, f-strings, and symbols
            _ => {
                if ch == 'f' && i + 1 < chars.len() && chars[i + 1] == '"' {
                    // f-string: f"Hello ${name}" → FString token
                    let token_start = i;
                    i += 1; // skip 'f'
                    col += 1;
                    i += 1; // skip opening '"'
                    col += 1;
                    let mut parts: Vec<FStringPart> = Vec::new();
                    let mut current = String::new();

                    while i < chars.len() && chars[i] != '"' {
                        if chars[i] == '\\' && i + 1 < chars.len() {
                            i += 1;
                            col += 1;
                            read_string_escape(&chars, &mut i, &mut col, &mut current, span)?;
                        } else if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                            // Start interpolation
                            if !current.is_empty() {
                                parts.push(FStringPart::Literal(std::mem::take(&mut current)));
                            }
                            i += 2; // skip "${"
                            col += 2;
                            let mut expr = String::new();
                            let mut depth = 1;
                            while i < chars.len() && depth > 0 {
                                if chars[i] == '{' {
                                    depth += 1;
                                } else if chars[i] == '}' {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                if chars[i] == '\n' {
                                    line += 1;
                                    col = 0;
                                }
                                expr.push(chars[i]);
                                i += 1;
                                col += 1;
                            }
                            if depth != 0 {
                                return Err(SemaError::Reader {
                                    message: "unterminated interpolation in f-string".to_string(),
                                    span,
                                }
                                .with_hint("add a closing `}` to end the ${...} interpolation"));
                            }
                            let trimmed = expr.trim().to_string();
                            if trimmed.is_empty() {
                                return Err(SemaError::Reader {
                                    message: "empty interpolation in f-string".to_string(),
                                    span,
                                }
                                .with_hint("${} must contain an expression, e.g. ${name}"));
                            }
                            parts.push(FStringPart::Expr(trimmed));
                            // i points to closing '}', outer i+=1 will skip past it
                        } else {
                            if chars[i] == '\n' {
                                line += 1;
                                col = 0;
                            }
                            current.push(chars[i]);
                        }
                        i += 1;
                        col += 1;
                    }

                    if i >= chars.len() {
                        return Err(SemaError::Reader {
                            message: "unterminated f-string".to_string(),
                            span,
                        }
                        .with_hint("add a closing `\"` to end the f-string"));
                    }
                    i += 1; // closing quote
                    col += 1;

                    if !current.is_empty() {
                        parts.push(FStringPart::Literal(current));
                    }

                    tokens.push(SpannedToken {
                        token: Token::FString(parts),
                        span: span.with_end(line, col),
                        byte_start: byte_offsets[token_start],
                        byte_end: byte_offsets[i],
                    });
                } else if ch == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                    // Negative number
                    let token_start = i;
                    let (tok, len) = read_number(&chars[i..], &span)?;
                    i += len;
                    col += len;
                    tokens.push(SpannedToken {
                        token: tok,
                        span: span.with_end(line, col),
                        byte_start: byte_offsets[token_start],
                        byte_end: byte_offsets[i],
                    });
                } else if ch.is_ascii_digit() {
                    let token_start = i;
                    let (tok, len) = read_number(&chars[i..], &span)?;
                    i += len;
                    col += len;
                    tokens.push(SpannedToken {
                        token: tok,
                        span: span.with_end(line, col),
                        byte_start: byte_offsets[token_start],
                        byte_end: byte_offsets[i],
                    });
                } else if is_symbol_start(ch) {
                    let start = i;
                    while i < chars.len() && is_symbol_char(chars[i]) {
                        i += 1;
                        col += 1;
                    }
                    let name: String = chars[start..i].iter().collect();
                    let token_span = span.with_end(line, col);
                    // Check for special symbol names
                    let token_byte_start = byte_offsets[start];
                    let token_byte_end = byte_offsets[i];
                    match name.as_str() {
                        "true" => tokens.push(SpannedToken {
                            token: Token::Bool(true),
                            span: token_span,
                            byte_start: token_byte_start,
                            byte_end: token_byte_end,
                        }),
                        "false" => tokens.push(SpannedToken {
                            token: Token::Bool(false),
                            span: token_span,
                            byte_start: token_byte_start,
                            byte_end: token_byte_end,
                        }),
                        "nil" => tokens.push(SpannedToken {
                            token: Token::Symbol("nil".to_string()),
                            span: token_span,
                            byte_start: token_byte_start,
                            byte_end: token_byte_end,
                        }),
                        "." => tokens.push(SpannedToken {
                            token: Token::Dot,
                            span: token_span,
                            byte_start: token_byte_start,
                            byte_end: token_byte_end,
                        }),
                        // IEEE 754 special floats, so they round-trip through the
                        // printer (which emits `inf` / `-inf` / `NaN`). Accept a
                        // few common spellings too.
                        "inf" | "+inf" | "Inf" | "Infinity" | "+Infinity" => {
                            tokens.push(SpannedToken {
                                token: Token::Float(f64::INFINITY),
                                span: token_span,
                                byte_start: token_byte_start,
                                byte_end: token_byte_end,
                            })
                        }
                        "-inf" | "-Infinity" => tokens.push(SpannedToken {
                            token: Token::Float(f64::NEG_INFINITY),
                            span: token_span,
                            byte_start: token_byte_start,
                            byte_end: token_byte_end,
                        }),
                        "nan" | "NaN" | "NAN" | "+nan" | "-nan" => tokens.push(SpannedToken {
                            token: Token::Float(f64::NAN),
                            span: token_span,
                            byte_start: token_byte_start,
                            byte_end: token_byte_end,
                        }),
                        _ => tokens.push(SpannedToken {
                            token: Token::Symbol(name),
                            span: token_span,
                            byte_start: token_byte_start,
                            byte_end: token_byte_end,
                        }),
                    }
                } else {
                    return Err(SemaError::Reader {
                        message: format!("unexpected character: '{ch}'"),
                        span,
                    });
                }
            }
        }
    }

    Ok(tokens)
}

/// Process a string escape sequence. `chars[*i]` is the character after `\`.
/// Pushes the decoded character(s) to `buf` and advances `*i`/`*col` for
/// multi-character escapes (hex, unicode). The caller handles the final `i += 1`.
fn read_string_escape(
    chars: &[char],
    i: &mut usize,
    col: &mut usize,
    buf: &mut String,
    span: Span,
) -> Result<(), SemaError> {
    match chars[*i] {
        'n' => buf.push('\n'),
        't' => buf.push('\t'),
        'r' => buf.push('\r'),
        '\\' => buf.push('\\'),
        '"' => buf.push('"'),
        '0' => buf.push('\0'),
        '$' => buf.push('$'),
        'x' => {
            // R7RS hex escape: \x<hex>;
            let mut hex = String::new();
            while *i + 1 < chars.len() && chars[*i + 1] != ';' && chars[*i + 1].is_ascii_hexdigit()
            {
                *i += 1;
                *col += 1;
                hex.push(chars[*i]);
            }
            if hex.is_empty() {
                return Err(SemaError::Reader {
                    message: "empty hex escape \\x;".to_string(),
                    span,
                });
            }
            if *i + 1 >= chars.len() || chars[*i + 1] != ';' {
                return Err(SemaError::Reader {
                    message: "hex escape \\x missing terminating semicolon".to_string(),
                    span,
                });
            }
            *i += 1;
            *col += 1;
            let code = u32::from_str_radix(&hex, 16).map_err(|_| SemaError::Reader {
                message: format!("invalid hex escape \\x{};", hex),
                span,
            })?;
            let ch = char::from_u32(code).ok_or_else(|| SemaError::Reader {
                message: format!("invalid unicode scalar value \\x{};", hex),
                span,
            })?;
            buf.push(ch);
        }
        'u' => {
            // \u<4 hex digits>
            let mut hex = String::new();
            for _ in 0..4 {
                if *i + 1 >= chars.len() || !chars[*i + 1].is_ascii_hexdigit() {
                    return Err(SemaError::Reader {
                        message: "\\u escape requires exactly 4 hex digits".to_string(),
                        span,
                    });
                }
                *i += 1;
                *col += 1;
                hex.push(chars[*i]);
            }
            let code = u32::from_str_radix(&hex, 16).map_err(|_| SemaError::Reader {
                message: format!("invalid hex escape \\u{}", hex),
                span,
            })?;
            let ch = char::from_u32(code).ok_or_else(|| SemaError::Reader {
                message: format!("invalid unicode scalar value \\u{}", hex),
                span,
            })?;
            buf.push(ch);
        }
        'U' => {
            // \U<8 hex digits>
            let mut hex = String::new();
            for _ in 0..8 {
                if *i + 1 >= chars.len() || !chars[*i + 1].is_ascii_hexdigit() {
                    return Err(SemaError::Reader {
                        message: "\\U escape requires exactly 8 hex digits".to_string(),
                        span,
                    });
                }
                *i += 1;
                *col += 1;
                hex.push(chars[*i]);
            }
            let code = u32::from_str_radix(&hex, 16).map_err(|_| SemaError::Reader {
                message: format!("invalid hex escape \\U{}", hex),
                span,
            })?;
            let ch = char::from_u32(code).ok_or_else(|| SemaError::Reader {
                message: format!("invalid unicode scalar value \\U{}", hex),
                span,
            })?;
            buf.push(ch);
        }
        other => {
            buf.push('\\');
            buf.push(other);
        }
    }
    Ok(())
}

fn read_number(chars: &[char], span: &Span) -> Result<(Token, usize), SemaError> {
    let mut i = 0;
    if chars[i] == '-' {
        i += 1;
    }
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    let mut is_float = false;
    // Fraction: a `.` followed by at least one digit (a trailing `.` is left to the
    // symbol lexer, preserving the existing `1.` behavior).
    if i < chars.len() && chars[i] == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
        i += 1; // skip dot
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        is_float = true;
    }
    // Exponent: `[eE] [+-]? digit+`. Only consumed when a digit actually follows
    // (after an optional sign), so a bare `1e`, `1e+`, or an identifier like `e19`
    // is NOT mis-lexed as a number — the `e`/`E` is left for the symbol lexer.
    // `f64::parse` accepts the resulting `<mantissa>e<exp>` string directly.
    if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
        let mut j = i + 1;
        if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
            j += 1;
        }
        if j < chars.len() && chars[j].is_ascii_digit() {
            i = j;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            is_float = true;
        }
    }
    let s: String = chars[..i].iter().collect();
    if is_float {
        let f: f64 = s.parse().map_err(|_| SemaError::Reader {
            message: format!("invalid float: {s}"),
            span: *span,
        })?;
        Ok((Token::Float(f), i))
    } else {
        let n: i64 = s.parse().map_err(|_| SemaError::Reader {
            message: format!("invalid integer: {s}"),
            span: *span,
        })?;
        Ok((Token::Int(n), i))
    }
}

fn is_symbol_start(ch: char) -> bool {
    ch.is_alphabetic()
        || matches!(
            ch,
            '+' | '-' | '*' | '/' | '!' | '?' | '<' | '>' | '=' | '_' | '&' | '%' | '^' | '~' | '.'
        )
}

fn is_symbol_char(ch: char) -> bool {
    is_symbol_start(ch) || ch.is_ascii_digit() || matches!(ch, '-' | '/' | '.' | '#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comment_token_emitted() {
        let tokens = tokenize("(+ 1 2) ; comment").unwrap();
        let comment_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(&t.token, Token::Comment(_)))
            .collect();
        assert_eq!(comment_tokens.len(), 1);
        match &comment_tokens[0].token {
            Token::Comment(text) => assert_eq!(text, "; comment"),
            _ => panic!("expected Comment token"),
        }
    }

    #[test]
    fn test_scientific_notation_literals() {
        let first = |src: &str| tokenize(src).unwrap().into_iter().next().unwrap().token;
        let toks = |src: &str| -> Vec<Token> {
            tokenize(src)
                .unwrap()
                .into_iter()
                .map(|t| t.token)
                .collect()
        };
        // Exponent forms parse as Float (f64::parse handles the string once consumed).
        assert_eq!(first("1.0e19"), Token::Float(1e19));
        assert_eq!(first("1e19"), Token::Float(1e19)); // bare exponent (no fraction)
        assert_eq!(first("1.5e3"), Token::Float(1500.0));
        assert_eq!(first("2e-5"), Token::Float(2e-5)); // signed (negative) exponent
        assert_eq!(first("6.022e+23"), Token::Float(6.022e23)); // explicit + sign
        assert_eq!(first("1E10"), Token::Float(1e10)); // uppercase E
        assert_eq!(first("1E+10"), Token::Float(1e10)); // uppercase E + sign
        assert_eq!(first("1E-10"), Token::Float(1e-10));
        assert_eq!(first("-1.5e3"), Token::Float(-1500.0)); // negative mantissa + fraction
        assert_eq!(first("-2e3"), Token::Float(-2000.0)); // negative bare-int mantissa
        assert_eq!(first("-1e-3"), Token::Float(-1e-3)); // negative mantissa AND exponent
                                                         // Out-of-range exponents follow IEEE-754 / `f64::parse` (matches the lexer
                                                         // already accepting `inf`/`nan` literals): overflow → inf, underflow → 0.0.
        assert_eq!(first("1e400"), Token::Float(f64::INFINITY));
        assert_eq!(first("1e-400"), Token::Float(0.0));
        // Plain integers/floats are unaffected.
        assert_eq!(first("42"), Token::Int(42));
        assert_eq!(first("1.5"), Token::Float(1.5));
        assert_eq!(first("-7"), Token::Int(-7));
        // The returned consumed-length must be exact (the caller advances i/col/byte
        // offsets by it): a number resumes correct tokenization of what follows, both
        // with a separating space and immediately against a delimiter.
        assert_eq!(toks("1e5 2"), vec![Token::Float(1e5), Token::Int(2)]);
        assert_eq!(
            toks("(f 1e5)"),
            vec![
                Token::LParen,
                Token::Symbol("f".to_string()),
                Token::Float(1e5),
                Token::RParen,
            ]
        );
        // Guards: an `e`/`E` not followed by (sign+)digits is NOT consumed — `1e`
        // is Int(1) plus a separate symbol, and a bare `e19` identifier stays a
        // symbol, so existing code using `e…` names is unaffected.
        assert_eq!(first("1e"), Token::Int(1));
        assert_eq!(toks("1e").len(), 2);
        assert!(matches!(&toks("1e")[1], Token::Symbol(s) if s == "e"));
        assert_eq!(first("1e+"), Token::Int(1));
        assert!(matches!(&toks("1e+")[1], Token::Symbol(_))); // leftover `e+` is a symbol
        assert!(matches!(first("e19"), Token::Symbol(s) if s == "e19"));
    }

    #[test]
    fn test_newline_token_emitted() {
        let tokens = tokenize("a\nb").unwrap();
        let token_types: Vec<_> = tokens.iter().map(|t| &t.token).collect();
        assert!(
            matches!(token_types[0], Token::Symbol(s) if s == "a"),
            "first token should be symbol 'a'"
        );
        assert!(
            matches!(token_types[1], Token::Newline),
            "second token should be Newline"
        );
        assert!(
            matches!(token_types[2], Token::Symbol(s) if s == "b"),
            "third token should be symbol 'b'"
        );
    }

    #[test]
    fn test_regex_token_emitted() {
        let tokens = tokenize(r#"#"\d+""#).unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0].token {
            Token::Regex(s) => assert_eq!(s, r"\d+"),
            other => panic!("expected Regex token, got {:?}", other),
        }
    }

    #[test]
    fn test_regex_not_string() {
        // Regex should NOT produce Token::String
        let tokens = tokenize(r#"#"[a-z]+""#).unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(
            !matches!(&tokens[0].token, Token::String(_)),
            "regex should not produce Token::String"
        );
        assert!(
            matches!(&tokens[0].token, Token::Regex(_)),
            "regex should produce Token::Regex"
        );
    }

    #[test]
    fn test_multiple_comments_and_newlines_preserved() {
        let tokens = tokenize("; first\n; second\n42").unwrap();
        let token_types: Vec<&Token> = tokens.iter().map(|t| &t.token).collect();
        assert!(matches!(token_types[0], Token::Comment(s) if s == "; first"));
        assert!(matches!(token_types[1], Token::Newline));
        assert!(matches!(token_types[2], Token::Comment(s) if s == "; second"));
        assert!(matches!(token_types[3], Token::Newline));
        assert!(matches!(token_types[4], Token::Int(42)));
    }

    #[test]
    fn test_comment_does_not_include_trailing_newline() {
        let tokens = tokenize("; hello world\n").unwrap();
        match &tokens[0].token {
            Token::Comment(text) => {
                assert!(
                    !text.ends_with('\n'),
                    "comment should not include trailing newline"
                );
                assert_eq!(text, "; hello world");
            }
            _ => panic!("expected Comment token"),
        }
        // The newline should be a separate token
        assert!(matches!(&tokens[1].token, Token::Newline));
    }

    #[test]
    fn test_inline_comment_after_code() {
        let tokens = tokenize("(define x 42) ; set x").unwrap();
        let has_comment = tokens
            .iter()
            .any(|t| matches!(&t.token, Token::Comment(s) if s == "; set x"));
        assert!(has_comment, "should have inline comment token");
    }

    #[test]
    fn test_trivia_order_preserved() {
        let tokens = tokenize("a\n\n; comment\nb").unwrap();
        let types: Vec<String> = tokens
            .iter()
            .map(|t| match &t.token {
                Token::Symbol(s) => format!("sym:{}", s),
                Token::Newline => "newline".to_string(),
                Token::Comment(s) => format!("comment:{}", s),
                other => format!("{:?}", other),
            })
            .collect();
        assert_eq!(
            types,
            vec![
                "sym:a",
                "newline",
                "newline",
                "comment:; comment",
                "newline",
                "sym:b"
            ]
        );
    }
}
