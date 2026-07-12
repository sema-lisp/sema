//! `,disasm EXPR` — compile an expression and print its bytecode.
//!
//! Reuses the same pipeline as the `sema disasm` CLI subcommand
//! (`crates/sema/src/main.rs::run_disasm`): read → macro-expand for VM →
//! `compile_program` → `disassemble`. No new compilation code.

use sema_eval::Interpreter;
use sema_reader::read_many;
use sema_vm::{compile_program, disassemble};

use crate::{colors, print_error};

pub fn run(interpreter: &Interpreter, expr_str: &str) {
    let expr_str = expr_str.trim();
    if expr_str.is_empty() {
        println!("Usage: ,disasm <expr>");
        return;
    }

    // Track the source for error rendering, same way ,type / ,time do.
    crate::LAST_SOURCE.with(|s| *s.borrow_mut() = Some(expr_str.to_string()));
    crate::LAST_FILE.with(|f| *f.borrow_mut() = None);

    let exprs = match read_many(expr_str) {
        Ok(v) => v,
        Err(e) => {
            print_error(&e);
            return;
        }
    };

    let expanded: Result<Vec<_>, _> = interpreter.expand_for_vm_batch(&exprs);
    let expanded = match expanded {
        Ok(v) => v,
        Err(e) => {
            print_error(&e);
            return;
        }
    };

    let program = match compile_program(&expanded, None) {
        Ok(p) => p,
        Err(e) => {
            print_error(&e);
            return;
        }
    };

    // Top-level chunk first.
    println!(
        "{}",
        disassemble(&program.closure.func.chunk, Some("<expr>"))
    );

    // Then any nested functions defined by lambdas / closures in the expr.
    for (i, func) in program.functions.iter().enumerate() {
        let label = match func.name {
            Some(spur) => sema_core::resolve(spur),
            None => format!("<fn#{i}>"),
        };
        println!();
        println!("{}", colors::dim(&format!("; function {label}")));
        println!("{}", disassemble(&func.chunk, Some(&label)));
    }
}
