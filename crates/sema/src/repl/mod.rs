//! Interactive Sema REPL built on `reedline`.
//!
//! Migrated from `rustyline` to `reedline` to enable richer features going
//! forward (typed StyledText highlighting, popup completion menu, native
//! multiline validator). The user-facing behaviour of this Phase-0 cut
//! matches the previous REPL: same prompts, same meta-commands, same
//! history slots (`*1`/`*2`/`*3`/`*e`), same error rendering.

use std::collections::HashSet;
use std::io::IsTerminal;

use reedline::Signal;
use sema_core::{intern, pretty_print, Spur, Value};
use sema_eval::Interpreter;

use crate::{drain_async_scheduler, print_error, LAST_FILE, LAST_SOURCE};

mod apropos;
mod commands;
mod completer;
mod disasm;
mod editor;
mod headless;
pub(crate) mod highlighter;
mod hinter;
mod history;
mod inspector;
mod prompt;
mod validator;

use commands::CommandOutcome;
use prompt::SemaPrompt;

/// Entry point for the interactive REPL.
pub fn run(interpreter: Interpreter, quiet: bool, sandbox_mode: Option<&str>) {
    let env = interpreter.global_env.clone();

    // Snapshot the prelude / builtin keys so `,env` can hide them and only
    // surface user-defined bindings.
    let mut prelude_keys: HashSet<Spur> = HashSet::new();
    env.iter_bindings(|spur, _| {
        prelude_keys.insert(spur);
    });

    // Initialise the history slot variables and mark them as prelude so they
    // don't leak into `,env`.
    for slot in ["*1", "*2", "*3", "*e"] {
        env.set(intern(slot), Value::nil());
        prelude_keys.insert(intern(slot));
    }
    interpreter.ctx.interactive.set(true);

    if !quiet {
        println!(
            "Sema v{} — A Lisp with LLM primitives",
            env!("CARGO_PKG_VERSION")
        );
        if let Some(mode) = sandbox_mode {
            println!("Sandbox: {mode}");
        }
        println!("Type ,help for help, ,quit to exit\n");
    }

    // Reedline puts the terminal in raw mode and queries the cursor; it
    // can't run against piped stdin (CI, test harnesses, `printf ... | sema`).
    // Fall back to the coordinated headless loop in that case.
    if !std::io::stdin().is_terminal() {
        match headless::run_stdin(&interpreter, &env, &prelude_keys) {
            Ok(()) => {
                println!("Goodbye!");
            }
            Err(msg) => {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
        }
        return;
    }

    completer::set_completer_env(env.clone());
    let mut line_editor = editor::build();
    let prompt = SemaPrompt;

    loop {
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }

                let outcome = commands::dispatch(&trimmed, &interpreter, &env, &prelude_keys);

                match outcome {
                    CommandOutcome::Quit => break,
                    CommandOutcome::Handled => continue,
                    CommandOutcome::Passthrough => {}
                }

                LAST_SOURCE.with(|s| *s.borrow_mut() = Some(trimmed.clone()));
                LAST_FILE.with(|f| *f.borrow_mut() = None);

                match interpreter.eval_str_in_global(&trimmed) {
                    Ok(val) => {
                        drain_async_scheduler(&interpreter);
                        rotate_result_slots(&env, val.clone());
                        if !val.is_nil() {
                            println!("{}", pretty_print(&val, 80));
                        } else if let Some(name) = top_level_define_name(&trimmed) {
                            println!("{}", crate::colors::dim(&format!("; defined {name}")));
                        }
                    }
                    Err(e) => {
                        env.set(intern("*e"), Value::string(&e.to_string()));
                        print_error(&e);
                    }
                }
            }
            Ok(Signal::CtrlC) => {
                // Reedline already clears the in-progress buffer on Ctrl-C
                // and re-prompts; we just acknowledge it.
                println!("^C");
                continue;
            }
            Ok(Signal::CtrlD) => {
                break;
            }
            Ok(_) => {
                // Reedline added a new Signal variant; treat as a no-op so we
                // don't crash on future versions.
                continue;
            }
            Err(e) => {
                eprintln!("Error: {e}");
                break;
            }
        }
    }

    println!("Goodbye!");
}

/// Roll `*1` → `*2` → `*3` (most recent first) and store the new value in `*1`.
pub(super) fn rotate_result_slots(env: &sema_core::Env, val: Value) {
    if let Some(v1) = env.get(intern("*1")) {
        if env.get(intern("*2")).is_some() {
            let v2 = env.get(intern("*2")).unwrap();
            env.set(intern("*3"), v2);
        }
        env.set(intern("*2"), v1);
    }
    env.set(intern("*1"), val);
}

/// If `input` is a top-level definition form like `(define x ...)`,
/// `(defun foo ...)`, or `(defmacro bar ...)`, return the defined name.
/// Returns `None` for anything else, including nested defines.
pub(super) fn top_level_define_name(input: &str) -> Option<String> {
    let trimmed = input.trim_start();
    for kw in ["(define", "(defun", "(defmacro"] {
        if let Some(rest) = trimmed.strip_prefix(kw) {
            let next = rest.chars().next()?;
            if !next.is_whitespace() && next != '(' {
                continue;
            }
            let mut after = rest.trim_start();
            if let Some(stripped) = after.strip_prefix('(') {
                after = stripped.trim_start();
            }
            let name: String = after
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '(' && *c != ')')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::top_level_define_name;

    #[test]
    fn detects_define() {
        assert_eq!(top_level_define_name("(define x 42)"), Some("x".into()));
        assert_eq!(
            top_level_define_name("(define (foo a b) (+ a b))"),
            Some("foo".into())
        );
        assert_eq!(
            top_level_define_name("(defun bar () 1)"),
            Some("bar".into())
        );
        assert_eq!(
            top_level_define_name("(defmacro baz (x) `(+ ,x 1))"),
            Some("baz".into())
        );
    }

    #[test]
    fn ignores_non_define() {
        assert_eq!(top_level_define_name("(+ 1 2)"), None);
        assert_eq!(top_level_define_name("(definer x 1)"), None);
        assert_eq!(top_level_define_name("(let ((x 1)) (define y 2))"), None);
    }
}
