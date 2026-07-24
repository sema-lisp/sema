//! Non-interactive REPL fallback used when stdin is not a TTY.
//!
//! Reedline (like any line editor) needs a real terminal: it puts the
//! terminal in raw mode and queries the cursor position. When the REPL is
//! driven by piped input (CI scripts, integration tests, batch jobs) those
//! probes fail and the editor aborts. To keep `sema` usable as
//! `printf '(+ 1 2)\n' | sema` we drop down to a plain BufRead loop here.
//!
//! The user-visible behaviour matches the interactive REPL where possible:
//! same banner (printed by the caller), same meta-commands, same history
//! slot rotation, same `; defined NAME` feedback, same "unterminated input
//! at EOF" exit on mid-form EOF.

use std::collections::HashSet;
use std::io::{self, BufRead, Read};

use sema_core::{intern, pretty_print, Spur, Value};
use sema_eval::Interpreter;

use crate::{drain_async_scheduler, print_error, LAST_FILE, LAST_SOURCE};

use super::commands::{self, CommandOutcome};
use super::validator::is_input_complete;

/// Drive the REPL from a generic reader (typically stdin). Returns whether
/// the session ended cleanly; the caller is responsible for printing the
/// "Goodbye!" line so we match the TTY path exactly.
///
/// `prelude_keys` is consumed: ownership keeps it from being aliased across
/// the interactive vs. headless paths and makes intent obvious.
pub fn run<R: BufRead>(
    mut input: R,
    interpreter: &Interpreter,
    env: &sema_core::Env,
    prelude_keys: &HashSet<Spur>,
) -> Result<(), String> {
    run_with_line_source(
        |line| {
            input
                .read_line(line)
                .map(|read| read != 0)
                .map_err(|error| format!("read error: {error}"))
        },
        interpreter,
        env,
        prelude_keys,
    )
}

/// Drive the native headless REPL through the coordinated stdin owner.
///
/// The owner may buffer bytes read from the OS, but this loop acquires only one
/// logical source line before evaluating it. Runtime stdin operations can then
/// take the next lease and consume any following data from that shared buffer.
pub fn run_stdin(
    interpreter: &Interpreter,
    env: &sema_core::Env,
    prelude_keys: &HashSet<Spur>,
) -> Result<(), String> {
    run(
        CoordinatedStdinReader::default(),
        interpreter,
        env,
        prelude_keys,
    )
}

#[derive(Default)]
struct CoordinatedStdinReader {
    line: Vec<u8>,
    consumed: usize,
    eof: bool,
}

impl Read for CoordinatedStdinReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let read = available.len().min(output.len());
        output[..read].copy_from_slice(&available[..read]);
        self.consume(read);
        Ok(read)
    }
}

impl BufRead for CoordinatedStdinReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.consumed == self.line.len() && !self.eof {
            self.line.clear();
            self.consumed = 0;
            match sema_stdlib::read_coordinated_stdin_source_line()
                .map_err(|error| io::Error::other(error.to_string()))?
            {
                Some(source_line) => {
                    self.line.extend_from_slice(source_line.as_bytes());
                    self.line.push(b'\n');
                }
                None => self.eof = true,
            }
        }
        Ok(&self.line[self.consumed..])
    }

    fn consume(&mut self, amount: usize) {
        self.consumed = self.line.len().min(self.consumed.saturating_add(amount));
    }
}

fn run_with_line_source(
    mut read_line: impl FnMut(&mut String) -> Result<bool, String>,
    interpreter: &Interpreter,
    env: &sema_core::Env,
    prelude_keys: &HashSet<Spur>,
) -> Result<(), String> {
    let mut buffer = String::new();
    let mut in_multiline = false;
    let mut line = String::new();

    loop {
        line.clear();
        if !read_line(&mut line)? {
            // EOF. If we were mid-form, surface that loudly so users
            // don't silently lose their input.
            if in_multiline || !buffer.trim().is_empty() {
                return Err(format!("unterminated input at EOF: {}", buffer.trim()));
            }
            return Ok(());
        }

        // Strip the trailing newline that read_line leaves behind, but
        // keep internal whitespace.
        let raw = line.trim_end_matches('\n').trim_end_matches('\r');

        if !in_multiline {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }

            let outcome = commands::dispatch(trimmed, interpreter, env, prelude_keys);
            match outcome {
                CommandOutcome::Quit => return Ok(()),
                CommandOutcome::Handled => continue,
                CommandOutcome::Passthrough => {}
            }
            buffer = raw.to_string();
        } else {
            buffer.push('\n');
            buffer.push_str(raw);
        }

        if !is_input_complete(&buffer) {
            in_multiline = true;
            continue;
        }

        in_multiline = false;
        let submitted = buffer.trim().to_string();
        buffer.clear();
        if submitted.is_empty() {
            continue;
        }

        LAST_SOURCE.with(|s| *s.borrow_mut() = Some(submitted.clone()));
        LAST_FILE.with(|f| *f.borrow_mut() = None);

        match interpreter.eval_str_in_global(&submitted) {
            Ok(val) => {
                drain_async_scheduler(interpreter);
                super::rotate_result_slots(env, val.clone());
                if !val.is_nil() {
                    println!("{}", pretty_print(&val, 80));
                } else if let Some(name) = super::top_level_define_name(&submitted) {
                    println!("{}", crate::colors::dim(&format!("; defined {name}")));
                }
            }
            Err(e) => {
                env.set(intern("*e"), Value::string(&e.to_string()));
                print_error(&e);
            }
        }
    }
}
