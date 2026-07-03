use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};
use sema_core::SemaError;
use sema_eval::SPECIAL_FORM_NAMES;
use sema_reader::lexer::{tokenize, SpannedToken, Token};

// Sema brand palette from `crates/sema/src/colors.rs` / `website/.vitepress/theme/BrandGuide.vue`.
const GOLD: Color = Color::Rgb(200, 168, 85);
const SAGE: Color = Color::Rgb(168, 196, 122);
const AMBER: Color = Color::Rgb(209, 154, 102);
const TEAL: Color = Color::Rgb(122, 172, 184);
const TERTIARY: Color = Color::Rgb(107, 99, 84);
const SECONDARY: Color = Color::Rgb(150, 140, 121);

/// Lexer-driven syntax highlighter for the Sema REPL input line.
///
/// The implementation is intentionally tolerant of half-typed input:
/// if `tokenize` fails (the user hasn't closed a string yet, etc.) the
/// prefix that did parse is still coloured and the rest of the line is
/// rendered as plain text. Reedline calls this on every keystroke, so
/// it must be cheap; the lexer is a single pass over `line`.
///
/// When the cursor sits on a bracket (or just past one), the matching
/// partner is bolded. A lonely bracket with no partner is coloured red.
pub(crate) struct SemaHighlighter;

impl SemaHighlighter {
    pub fn new() -> Self {
        Self
    }
}

impl Highlighter for SemaHighlighter {
    fn highlight(&self, line: &str, cursor: usize) -> StyledText {
        let mut out = StyledText::new();

        let tokens = match tokenize(line) {
            Ok(t) => t,
            Err(SemaError::Reader { .. }) => {
                // Half-typed line. Best-effort: emit the whole buffer
                // unstyled rather than crashing reedline's render pass.
                out.push((Style::default(), line.to_string()));
                return out;
            }
            Err(_) => {
                out.push((Style::default(), line.to_string()));
                return out;
            }
        };

        // Bracket-matching: figure out which bracket pair the cursor is
        // touching so we can bold both partners.
        let (match_a, match_b) = matching_bracket_indices(&tokens, cursor);

        let mut last_end: usize = 0;
        let mut next_symbol_is_fn = false;
        for (idx, tok) in tokens.iter().enumerate() {
            // Emit any whitespace between the previous token and this one
            // as plain text so cursor positioning stays accurate.
            if tok.byte_start > last_end {
                out.push((Style::default(), line[last_end..tok.byte_start].to_string()));
            }

            let is_fn_position = next_symbol_is_fn && matches!(tok.token, Token::Symbol(_));
            if is_fn_position {
                next_symbol_is_fn = false;
            }

            let segment = &line[tok.byte_start..tok.byte_end];
            let style = style_for(&tok.token, idx, match_a, match_b, is_fn_position);
            out.push((style, segment.to_string()));

            if matches!(tok.token, Token::LParen | Token::ShortLambdaStart) {
                next_symbol_is_fn = true;
            }

            last_end = tok.byte_end;
        }

        // Trailing whitespace / unconsumed tail.
        if last_end < line.len() {
            out.push((Style::default(), line[last_end..].to_string()));
        }

        out
    }
}

fn style_for(
    token: &Token,
    idx: usize,
    match_a: Option<usize>,
    match_b: Option<usize>,
    is_fn_position: bool,
) -> Style {
    let is_match_partner = Some(idx) == match_a || Some(idx) == match_b;

    let base = match token {
        Token::String(_) | Token::FString(_) | Token::Char(_) => Style::new().fg(SAGE),
        Token::Regex(_) => Style::new().fg(SECONDARY),
        Token::Int(_) | Token::Float(_) | Token::Bool(_) => Style::new().fg(AMBER),
        Token::Keyword(_) => Style::new().fg(TEAL),
        Token::Comment(_) => Style::new().fg(TERTIARY),
        Token::Symbol(name) => {
            if SPECIAL_FORM_NAMES.contains(&name.as_str()) {
                Style::new().fg(GOLD).bold()
            } else if is_fn_position {
                // First symbol after '(' (or '#(') is the called function/operator.
                Style::new().fg(GOLD)
            } else {
                Style::default()
            }
        }
        Token::Quote
        | Token::Quasiquote
        | Token::Unquote
        | Token::UnquoteSplice
        | Token::Deref
        | Token::Dot => Style::new().fg(TERTIARY),
        Token::ShortLambdaStart | Token::BytevectorStart => Style::new().fg(GOLD),
        Token::LParen
        | Token::RParen
        | Token::LBracket
        | Token::RBracket
        | Token::LBrace
        | Token::RBrace => Style::default(),
        Token::Newline => Style::default(),
    };

    if is_match_partner {
        base.bold()
    } else {
        base
    }
}

pub(crate) fn highlight_sema_ansi(line: &str) -> String {
    let fence_style = Style::new().fg(TERTIARY);
    let highlighter = SemaHighlighter::new();
    let mut out = String::with_capacity(line.len() * 2);
    if line.trim_start().starts_with("```") {
        return fence_style.paint(line).to_string();
    }
    let styled = highlighter.highlight(line, 0);
    for (style, text) in &styled.buffer {
        out.push_str(&style.paint(text).to_string());
    }
    out
}

/// Highlight Sema code blocks inside a Markdown doc string.
#[cfg(test)]
pub(crate) fn highlight_doc_markdown(md: &str) -> String {
    md.to_string()
}

/// Given the tokens of the buffer and the cursor position, return the
/// indices of the bracket pair the cursor is touching, if any.
///
/// "Touching" means the cursor is either inside the bracket character or
/// sits just past it (cursor == byte_end), so users see the match light
/// up when they've just typed the bracket and when they navigate onto
/// it.
fn matching_bracket_indices(
    tokens: &[SpannedToken],
    cursor: usize,
) -> (Option<usize>, Option<usize>) {
    let Some(idx) = bracket_at_cursor(tokens, cursor) else {
        return (None, None);
    };

    let partner = match tokens[idx].token {
        Token::LParen | Token::ShortLambdaStart | Token::BytevectorStart => {
            find_partner_forward(tokens, idx, &[Token::RParen])
        }
        Token::LBracket => find_partner_forward(tokens, idx, &[Token::RBracket]),
        Token::LBrace => find_partner_forward(tokens, idx, &[Token::RBrace]),
        Token::RParen => find_partner_backward(
            tokens,
            idx,
            &[
                Token::LParen,
                Token::ShortLambdaStart,
                Token::BytevectorStart,
            ],
        ),
        Token::RBracket => find_partner_backward(tokens, idx, &[Token::LBracket]),
        Token::RBrace => find_partner_backward(tokens, idx, &[Token::LBrace]),
        _ => None,
    };

    (Some(idx), partner)
}

fn bracket_at_cursor(tokens: &[SpannedToken], cursor: usize) -> Option<usize> {
    tokens.iter().position(|t| {
        let is_bracket = matches!(
            t.token,
            Token::LParen
                | Token::RParen
                | Token::LBracket
                | Token::RBracket
                | Token::LBrace
                | Token::RBrace
                | Token::ShortLambdaStart
                | Token::BytevectorStart
        );
        is_bracket && (cursor >= t.byte_start && cursor <= t.byte_end)
    })
}

fn is_opener(t: &Token) -> bool {
    matches!(
        t,
        Token::LParen
            | Token::LBracket
            | Token::LBrace
            | Token::ShortLambdaStart
            | Token::BytevectorStart
    )
}

fn is_closer(t: &Token) -> bool {
    matches!(t, Token::RParen | Token::RBracket | Token::RBrace)
}

fn find_partner_forward(tokens: &[SpannedToken], from: usize, accept: &[Token]) -> Option<usize> {
    let mut depth: i32 = 0;
    for (i, tok) in tokens.iter().enumerate().skip(from) {
        if is_opener(&tok.token) {
            depth += 1;
        } else if is_closer(&tok.token) {
            depth -= 1;
            if depth == 0 {
                return if accept.iter().any(|a| same_kind(a, &tok.token)) {
                    Some(i)
                } else {
                    None
                };
            }
        }
    }
    None
}

fn find_partner_backward(tokens: &[SpannedToken], from: usize, accept: &[Token]) -> Option<usize> {
    let mut depth: i32 = 0;
    for i in (0..=from).rev() {
        let tok = &tokens[i];
        if is_closer(&tok.token) {
            depth += 1;
        } else if is_opener(&tok.token) {
            depth -= 1;
            if depth == 0 {
                return if accept.iter().any(|a| same_kind(a, &tok.token)) {
                    Some(i)
                } else {
                    None
                };
            }
        }
    }
    None
}

fn same_kind(a: &Token, b: &Token) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(line: &str, cursor: usize) -> Vec<(String, String)> {
        let styled = SemaHighlighter.highlight(line, cursor);
        styled
            .buffer
            .iter()
            .map(|(s, t)| (format!("{:?}", s), t.clone()))
            .collect()
    }

    #[test]
    fn highlights_string_literal() {
        let styled = SemaHighlighter.highlight("\"hi\"", 0);
        let pieces: Vec<&str> = styled.buffer.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(pieces.concat(), "\"hi\"");
    }

    #[test]
    fn highlights_keyword() {
        let styled = SemaHighlighter.highlight(":foo", 0);
        let pieces: Vec<&str> = styled.buffer.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(pieces.concat(), ":foo");
    }

    #[test]
    fn round_trips_input_unchanged_in_concat() {
        // Whatever styling we apply, the concatenated rendered text must
        // equal the original line — otherwise reedline's cursor math goes
        // wrong on the next keystroke.
        let cases = [
            "(+ 1 2)",
            "(define x \"hi\")",
            "; comment\n(+ 1 2)",
            "(define x (+ 1 (* 2 3)))",
            "#\"^[abc]\"",
            "f\"hello ${name}\"",
        ];
        for input in cases {
            let styled = SemaHighlighter.highlight(input, 0);
            let concat: String = styled.buffer.iter().map(|(_, s)| s.as_str()).collect();
            assert_eq!(concat, input, "round-trip failed for: {input:?}");
        }
    }

    #[test]
    fn tolerates_half_typed_input() {
        // Unterminated string — must not panic, must return something that
        // concatenates back to the original.
        let styled = SemaHighlighter.highlight("(define x \"abc", 0);
        let concat: String = styled.buffer.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(concat, "(define x \"abc");
    }

    #[test]
    fn bracket_match_pair_found() {
        // Cursor at column 0 is on the opening `(`. The partner should be
        // the closing `)` at column 6.
        let line = "(+ 1 2)";
        let tokens = tokenize(line).unwrap();
        let (a, b) = matching_bracket_indices(&tokens, 0);
        assert!(a.is_some());
        assert!(b.is_some());
        // Token indices: 0=(  1=+  2=1  3=2  4=)
        assert_eq!(a.unwrap(), 0);
        assert_eq!(b.unwrap(), 4);
    }

    #[test]
    fn bracket_match_handles_cursor_past_closer() {
        // Cursor just past the closing `)` (cursor == byte_end).
        let line = "(+ 1 2)";
        let tokens = tokenize(line).unwrap();
        let (a, b) = matching_bracket_indices(&tokens, line.len());
        assert!(a.is_some());
        assert!(b.is_some());
    }

    #[test]
    fn bracket_match_no_partner_when_cursor_not_on_bracket() {
        let line = "(+ 1 2)";
        let tokens = tokenize(line).unwrap();
        // Cursor on '+' (byte index 1) — not on a bracket.
        let (a, b) = matching_bracket_indices(&tokens, 2);
        assert!(a.is_none(), "got match {:?}", a);
        assert!(b.is_none());
        let _ = raw; // silence unused-helper warning
    }

    #[test]
    fn function_position_symbol_is_gold() {
        let styled = SemaHighlighter.highlight("(agent/name greeter)", 0);
        let gold = "\x1b[38;2;200;168;85m";
        let agent_name = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "agent/name")
            .expect("agent/name segment missing");
        let rendered = agent_name.0.paint("agent/name").to_string();
        assert!(
            rendered.starts_with(gold),
            "expected agent/name to be gold, got {rendered:?}"
        );
    }

    #[test]
    fn function_argument_symbol_stays_plain() {
        let styled = SemaHighlighter.highlight("(agent/name greeter)", 0);
        let greeter = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "greeter")
            .expect("greeter segment missing");
        let rendered = greeter.0.paint("greeter").to_string();
        assert_eq!(
            rendered, "greeter",
            "expected argument symbol to be unstyled, got {rendered:?}"
        );
    }

    #[test]
    fn special_form_is_bold_gold() {
        let styled = SemaHighlighter.highlight("(if #t 1 2)", 0);
        let bold_gold = "\x1b[1;38;2;200;168;85m";
        let if_token = styled
            .buffer
            .iter()
            .find(|(_, text)| text == "if")
            .expect("if segment missing");
        let rendered = if_token.0.paint("if").to_string();
        assert!(
            rendered.starts_with(bold_gold),
            "expected special form to be bold gold, got {rendered:?}"
        );
    }

    #[test]
    fn nested_function_calls_are_colored() {
        let styled = SemaHighlighter.highlight("(foo (bar x))", 0);
        let gold = "\x1b[38;2;200;168;85m";
        for name in ["foo", "bar"] {
            let seg = styled
                .buffer
                .iter()
                .find(|(_, text)| text == name)
                .expect("{name} segment missing");
            let rendered = seg.0.paint(name).to_string();
            assert!(
                rendered.starts_with(gold),
                "expected {name} to be gold, got {rendered:?}"
            );
        }
    }

    #[test]
    fn bracket_match_nested() {
        let line = "(let ((x 1)) x)";
        let tokens = tokenize(line).unwrap();
        // Cursor on outermost '(' at byte 0.
        let (a, b) = matching_bracket_indices(&tokens, 0);
        let a_idx = a.unwrap();
        let b_idx = b.unwrap();
        assert!(matches!(tokens[a_idx].token, Token::LParen));
        assert!(matches!(tokens[b_idx].token, Token::RParen));
        // The closer of the outermost paren should be the last token.
        assert_eq!(b_idx, tokens.len() - 1);
    }

    #[test]
    fn doc_markdown_passes_through_plain_text() {
        let md = "# Title\n\nSome body text.\n";
        // In the test harness stdout is not a terminal, so highlighting is disabled
        // and the markdown is returned unchanged.
        assert_eq!(highlight_doc_markdown(md), md);
    }

    #[test]
    fn doc_markdown_leaves_non_sema_blocks_plain() {
        let md = "```bash\necho hi\n```\n";
        assert_eq!(highlight_doc_markdown(md), md);
    }

    #[test]
    fn doc_markdown_dims_all_sema_fences() {
        let md = "Example:\n\n```sema\n(a 1)\n```\n\nMore:\n\n```sema\n(b 2)\n```\n";
        let out = crate::docs::render_terminal_markdown_inner(md, true);

        // Every fence line should start with the tertiary foreground escape.
        // (Lines begin with ANSI escapes after dimming, so we look for the
        // fence marker anywhere in the line — code lines never contain triple
        // backticks.)
        let tertiary = "\x1b[38;2;107;99;84m";
        let mut fence_count = 0;
        for line in out.lines() {
            if line.contains("```") {
                assert!(
                    line.starts_with(tertiary),
                    "fence line was not dimmed: {line:?}"
                );
                fence_count += 1;
            }
        }

        // Sanity: there are four fences (two open, two close).
        assert_eq!(fence_count, 4);
    }

    #[test]
    fn doc_markdown_defagent_closing_fence_is_dimmed() {
        // Exact body of crates/sema-docs/entries/special-forms/defagent.md,
        // trimmed the same way render_markdown does. This guards against a
        // regression where the final closing fence of a multi-block doc was
        // not styled.
        let md = "Define an LLM agent. The `name` must be a symbol.\n\n```sema\n(defagent greeter\n  {:system \"You are a friendly greeter.\"})\n```\n\nInspecting an agent:\n\n```sema\n(agent/name greeter)       ; => \"greeter\"\n(agent? greeter)           ; => #t\n```";
        let out = crate::docs::render_terminal_markdown_inner(md, true);

        let tertiary = "\x1b[38;2;107;99;84m";
        let mut fence_count = 0;
        for line in out.lines() {
            if line.contains("```") {
                assert!(line.starts_with(tertiary), "fence not dimmed: {line:?}");
                fence_count += 1;
            }
        }
        assert_eq!(fence_count, 4);
    }

    #[test]
    fn doc_markdown_last_block_highlights_function_calls() {
        // Regression: code in the final ```sema block of defagent was not
        // getting function-position syntax highlighting.
        let md = "Inspecting an agent:\n\n```sema\n(agent/name greeter)       ; => \"greeter\"\n(agent/system greeter)     ; => \"You are a friendly greeter...\"\n(agent/max-turns greeter)  ; => 5\n(agent? greeter)           ; => #t\n```";
        let out = crate::docs::render_terminal_markdown_inner(md, true);

        let gold = "\x1b[38;2;200;168;85m";
        for line in out.lines() {
            if line.contains("agent/") || line.contains("agent?") {
                // Each code line should begin with the gold escape from the
                // leading '(' or directly from the function symbol.
                assert!(
                    line.contains(gold),
                    "function call not highlighted in line: {line:?}"
                );
            }
        }
    }
}
