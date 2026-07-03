use std::collections::HashSet;

use sema_core::{pretty_print, Env, Spur, ValueView};
use sema_eval::{Interpreter, SPECIAL_FORM_NAMES};

use crate::{colors, docs, print_error, LAST_FILE, LAST_SOURCE};

pub const REPL_COMMANDS: &[&str] = &[
    ",quit",
    ",exit",
    ",q",
    ",help",
    ",h",
    ",env",
    ",builtins",
    ",gc",
    ",type",
    ",time",
    ",doc",
    ",disasm",
    ",apropos",
    ",inspect",
];

/// Outcome of dispatching a meta-command line.
pub enum CommandOutcome {
    /// Line was a meta-command and has been fully handled — caller should
    /// continue the outer loop.
    Handled,
    /// Caller should treat this as a `,quit`-style exit and break the loop.
    Quit,
    /// Line was not a meta-command; caller should evaluate it as Sema source.
    Passthrough,
}

pub fn dispatch(
    line: &str,
    interpreter: &Interpreter,
    env: &Env,
    prelude_keys: &HashSet<Spur>,
) -> CommandOutcome {
    let trimmed = line.trim();

    match trimmed {
        ",quit" | ",exit" | ",q" | "quit" | "exit" | ":q" => return CommandOutcome::Quit,
        ",help" | ",h" => {
            print_help();
            return CommandOutcome::Handled;
        }
        ",env" => {
            print_env(interpreter, prelude_keys);
            return CommandOutcome::Handled;
        }
        ",builtins" => {
            print_builtins(interpreter);
            return CommandOutcome::Handled;
        }
        ",gc" => {
            run_gc(interpreter);
            return CommandOutcome::Handled;
        }
        _ => {}
    }

    if matches!(
        trimmed,
        ",doc" | ",type" | ",time" | ",disasm" | ",apropos" | ",inspect"
    ) {
        println!("Usage: {trimmed} <expr>");
        return CommandOutcome::Handled;
    }

    if let Some(stripped) = trimmed.strip_prefix(",doc ") {
        doc(env, stripped.trim());
        return CommandOutcome::Handled;
    }

    if let Some(rest) = trimmed.strip_prefix(",disasm ") {
        super::disasm::run(interpreter, rest);
        return CommandOutcome::Handled;
    }

    if let Some(rest) = trimmed.strip_prefix(",apropos ") {
        super::apropos::run(env, rest);
        return CommandOutcome::Handled;
    }

    if let Some(rest) = trimmed.strip_prefix(",inspect ") {
        let rest = rest.trim();
        record_source(rest);
        match interpreter.eval_str_in_global(rest) {
            Ok(val) => {
                if let Err(e) = super::inspector::run(val, rest) {
                    eprintln!("inspector error: {e}");
                }
            }
            Err(e) => print_error(&e),
        }
        return CommandOutcome::Handled;
    }

    if let Some(expr) = trimmed.strip_prefix(",type ") {
        record_source(expr);
        match interpreter.eval_str_in_global(expr) {
            Ok(val) => {
                let type_name = match val.view() {
                    ValueView::Record(r) => format!(":{}", sema_core::resolve(r.type_tag)),
                    _ => format!(":{}", val.type_name()),
                };
                println!("{}", colors::dim(&type_name));
            }
            Err(e) => print_error(&e),
        }
        return CommandOutcome::Handled;
    }

    if let Some(expr) = trimmed.strip_prefix(",time ") {
        record_source(expr);
        let start = std::time::Instant::now();
        match interpreter.eval_str_in_global(expr) {
            Ok(val) => {
                let elapsed = start.elapsed();
                if !val.is_nil() {
                    println!("{}", pretty_print(&val, 80));
                }
                eprintln!("{} {elapsed:.3?}", colors::dim("elapsed:"));
            }
            Err(e) => {
                let elapsed = start.elapsed();
                print_error(&e);
                eprintln!("{} {elapsed:.3?}", colors::dim("elapsed:"));
            }
        }
        return CommandOutcome::Handled;
    }

    CommandOutcome::Passthrough
}

/// `,gc` — run a full cycle collection (CORE-2) and print its stats plus the
/// remaining candidate-registry size. Same pass as the `(gc/collect)` builtin,
/// pinned to skip descent into the live REPL namespace.
fn run_gc(interpreter: &Interpreter) {
    let pins = sema_core::gc_env_chain_pins(&interpreter.global_env);
    let stats = sema_core::gc_collect(&pins, sema_core::GcTrigger::Explicit);
    if stats.aborted {
        println!("gc: pass aborted (cell borrowed or collection already running)");
        return;
    }
    println!(
        "gc: collected {} of {} traced ({} candidates, {} registry entries pruned)",
        stats.collected, stats.traced, stats.candidates, stats.pruned
    );
    println!(
        "{}",
        colors::dim(&format!(
            "    registry: {} live candidates",
            sema_core::gc_registry_len()
        ))
    );
}

fn record_source(expr: &str) {
    LAST_SOURCE.with(|s| *s.borrow_mut() = Some(expr.to_string()));
    LAST_FILE.with(|f| *f.borrow_mut() = None);
}

fn doc(env: &Env, name: &str) {
    let spur = sema_core::intern(name);
    match env.get(spur) {
        Some(val) => {
            // VM closures arrive wrapped as NativeFn — peel that first so we
            // print a real arity line instead of a generic "native-fn".
            if let Some((closure, _funcs)) = sema_vm::extract_vm_closure(&val) {
                let arity = closure.func.arity;
                let rest = if closure.func.has_rest { " . rest" } else { "" };
                let params: Vec<String> = (0..arity).map(|i| format!("arg{i}")).collect();
                println!(
                    "  {} {} lambda ({}{})",
                    colors::cyan(name),
                    colors::dim(":"),
                    params.join(" "),
                    rest
                );
            } else {
                match val.view() {
                    ValueView::NativeFn(_f) => {
                        if let Some(rendered) = docs::rendered_doc(name) {
                            print!("{rendered}");
                        } else {
                            println!("  {} {} native-fn", colors::cyan(name), colors::dim(":"));
                        }
                    }
                    ValueView::Lambda(l) => {
                        let params: Vec<String> =
                            l.params.iter().map(|s| sema_core::resolve(*s)).collect();
                        let rest = l
                            .rest_param
                            .map(|s| format!(" . {}", sema_core::resolve(s)))
                            .unwrap_or_default();
                        println!(
                            "  {} {} lambda ({}{})",
                            colors::cyan(name),
                            colors::dim(":"),
                            params.join(" "),
                            rest
                        );
                    }
                    _ => {
                        println!(
                            "  {} {} {} = {}",
                            colors::cyan(name),
                            colors::dim(":"),
                            val.type_name(),
                            val
                        );
                    }
                }
            }
        }
        None => {
            if let Some(entry) = docs::lookup(name) {
                print!("{}", docs::rendered_doc_entry(name, entry));
            } else if SPECIAL_FORM_NAMES.contains(&name) {
                if let Some(rendered) = docs::rendered_doc(name) {
                    print!("{rendered}");
                } else {
                    eprintln!("  {} {name}", colors::red_bold("not found:"));
                }
            } else {
                eprintln!("  {} {name}", colors::red_bold("not found:"));
            }
        }
    }
}

fn print_help() {
    println!("Sema REPL Commands:");
    println!("  ,quit / ,q       Exit the REPL");
    println!("  ,help / ,h       Show this help");
    println!("  ,env             Show defined variables");
    println!("  ,builtins        List all builtin functions");
    println!("  ,gc              Run a cycle collection and show its stats");
    println!("  ,type EXPR       Show the type of a value");
    println!("  ,time EXPR       Evaluate and show elapsed time");
    println!("  ,doc NAME        Show info about a binding");
    println!("  ,apropos PAT     Search names by pattern (substring + fuzzy)");
    println!("  ,disasm EXPR     Compile EXPR and print its bytecode");
    println!("  ,inspect EXPR    Interactive arrow-key inspector for a value");
    println!();
    println!("LLM Quick Start:");
    println!("  Set ANTHROPIC_API_KEY or OPENAI_API_KEY env var, then:");
    println!("  (llm/complete \"Hello!\")");
    println!("  (llm/chat [(message :user \"Hi\")] {{:model \"claude-haiku-4-5-20251001\"}})");
    println!();
    println!("History Variables:");
    println!("  *1, *2, *3   Last three results (most recent first)");
    println!("  *e           Last error message");
    println!();
    println!("Core Forms:");
    println!("  define/defun, lambda/fn, if, cond, let, let*, begin/do");
    println!("  quote, quasiquote, defmacro, and, or, when, unless");
}

fn print_env(interpreter: &Interpreter, prelude_keys: &HashSet<Spur>) {
    let mut user_bindings: Vec<(String, String)> = Vec::new();
    interpreter.global_env.iter_bindings(|spur, val| {
        if val.as_native_fn_rc().is_some() {
            return;
        }
        if prelude_keys.contains(&spur) {
            return;
        }
        let name = sema_core::resolve(spur);
        // Hide history slots (*1, *2, *3, *e).
        if name.starts_with('*') {
            return;
        }
        user_bindings.push((name, format!("{val}")));
    });
    user_bindings.sort_by(|(a, _), (b, _)| a.cmp(b));
    if user_bindings.is_empty() {
        println!("(no user-defined bindings)");
    } else {
        for (name, val) in &user_bindings {
            println!("  {name} = {val}");
        }
    }
}

fn print_builtins(interpreter: &Interpreter) {
    let mut names: Vec<String> = Vec::new();
    interpreter.global_env.iter_bindings(|spur, val| {
        if val.as_native_fn_rc().is_some() {
            names.push(sema_core::resolve(spur));
        }
    });
    names.sort();

    if names.is_empty() {
        println!("(no builtin functions)");
        return;
    }

    let max_width = names.iter().map(|n| n.len()).max().unwrap_or(0) + 2;
    let term_width = 80;
    let cols = (term_width / max_width).max(1);

    for chunk in names.chunks(cols) {
        for name in chunk {
            print!("{name:<max_width$}");
        }
        println!();
    }
    println!("\n{} builtin functions", names.len());
}
