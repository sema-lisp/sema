use std::cell::RefCell;
use std::rc::Rc;

use reedline::{Completer, CompletionResult, Span, Suggestion};
use sema_core::Env;
use sema_eval::SPECIAL_FORM_NAMES;

use super::commands::REPL_COMMANDS;

// `reedline::Completer: Send`, but Sema's `Env` is built on `Rc<RefCell<_>>`
// and is fundamentally !Send. The REPL only ever runs on a single thread
// (the binary is single-threaded), so we stash the env in a thread-local
// and give the completer a zero-sized struct that is trivially Send.
// The hinter (sibling module) reads from this same slot to power the
// completion-based fallback for ghost text.
thread_local! {
    pub(super) static COMPLETER_ENV: RefCell<Option<Rc<Env>>> = const { RefCell::new(None) };
}

pub fn set_completer_env(env: Rc<Env>) {
    COMPLETER_ENV.with(|cell| *cell.borrow_mut() = Some(env));
}

/// Completes Sema symbols, special forms, and REPL meta-commands.
pub struct SemaCompleter;

impl SemaCompleter {
    pub fn new() -> Self {
        Self
    }

    fn collect(&self, prefix: &str) -> Vec<String> {
        COMPLETER_ENV.with(|cell| {
            let borrow = cell.borrow();
            match borrow.as_ref() {
                Some(env) => collect_completions(env, prefix),
                None => special_and_meta_completions(prefix),
            }
        })
    }
}

/// Gather all completions (env bindings + special forms + `,commands`)
/// for `prefix`, sorted and deduplicated. Exposed so the hinter can
/// reuse the same name set for completion-based ghost text.
pub(super) fn collect_completions(env: &Env, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_env(env, prefix, &mut out);
    extend_with_meta(prefix, &mut out);
    out.sort();
    out.dedup();
    out
}

fn special_and_meta_completions(prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    extend_with_meta(prefix, &mut out);
    out.sort();
    out.dedup();
    out
}

fn extend_with_meta(prefix: &str, out: &mut Vec<String>) {
    for &sf in SPECIAL_FORM_NAMES {
        if sf.starts_with(prefix) {
            out.push(sf.to_string());
        }
    }
    if prefix.starts_with(',') {
        for &cmd in REPL_COMMANDS {
            if cmd.starts_with(prefix) {
                out.push(cmd.to_string());
            }
        }
    }
}

fn collect_env(env: &Env, prefix: &str, out: &mut Vec<String>) {
    env.iter_bindings(|spur, _| {
        let name = sema_core::resolve(spur);
        if name.starts_with(prefix) {
            out.push(name);
        }
    });
    if let Some(parent) = &env.parent {
        collect_env(parent, prefix, out);
    }
}

impl Completer for SemaCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let before = &line[..pos];
        // A completion "word" is anything since the last whitespace or opening
        // bracket / quote.
        let start = before
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == '[' || c == '{' || c == '\'')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = &before[start..];
        if prefix.is_empty() {
            return CompletionResult::fresh(vec![]);
        }

        let names = self.collect(prefix);
        CompletionResult::fresh(
            names
                .into_iter()
                .map(|name| Suggestion {
                    value: name,
                    description: None,
                    style: None,
                    extra: None,
                    span: Span::new(start, pos),
                    append_whitespace: false,
                    display_override: None,
                    match_indices: None,
                })
                .collect::<Vec<_>>(),
        )
    }
}
