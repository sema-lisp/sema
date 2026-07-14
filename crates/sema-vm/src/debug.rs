use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc;

use sema_core::Value;

#[derive(Debug, Clone)]
pub struct DapStackFrame {
    pub id: u64,
    pub name: String,
    pub line: u64,
    pub column: u64,
    pub source_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DapVariable {
    pub name: String,
    pub value: String,
    pub type_name: String,
    pub variables_reference: u64,
}

#[derive(Debug, Clone)]
pub struct DapScope {
    pub name: String,
    pub variables_reference: u64,
    pub expensive: bool,
}

#[derive(Debug, Clone)]
pub struct DapBreakpoint {
    pub id: u32,
    pub verified: bool,
    pub requested_line: u32,
    pub line: u32,
    pub message: Option<String>,
}

/// A source breakpoint as requested by the frontend: a line plus an optional
/// condition expression that must evaluate truthy for the breakpoint to fire.
#[derive(Debug, Clone)]
pub struct SourceBreakpoint {
    pub line: u32,
    pub condition: Option<String>,
}

pub const DEBUG_VALUE_REF_BASE: u64 = 1_000_000;

/// Current stepping mode for the debugger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMode {
    /// Run until a breakpoint is hit.
    Continue,
    /// Stop at the next source line change (any frame depth).
    StepInto,
    /// Stop at the next source line change in the same or parent frame.
    StepOver,
    /// Stop when returning to the parent frame.
    StepOut,
}

/// Commands sent from the DAP frontend to the VM backend.
pub enum DebugCommand {
    Continue,
    StepInto,
    StepOver,
    StepOut,
    Pause,
    SetBreakpoints {
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
        reply: mpsc::SyncSender<Vec<DapBreakpoint>>,
    },
    /// Toggle stopping on uncaught runtime errors.
    SetExceptionBreakpoints {
        break_on_uncaught: bool,
    },
    GetStackTrace {
        reply: mpsc::SyncSender<Vec<DapStackFrame>>,
    },
    GetScopes {
        frame_id: usize,
        reply: mpsc::SyncSender<Vec<DapScope>>,
    },
    GetVariables {
        reference: u64,
        reply: mpsc::SyncSender<Vec<DapVariable>>,
    },
    Evaluate {
        frame_id: usize,
        expression: String,
        reply: mpsc::SyncSender<Result<DapVariable, String>>,
    },
    SetVariable {
        variables_reference: u64,
        name: String,
        value_expression: String,
        reply: mpsc::SyncSender<Result<DapVariable, String>>,
    },
    Disconnect,
}

/// Decoded scope variable reference.
pub enum ScopeKind {
    Locals(usize),
    Upvalues(usize),
}

/// Encode a locals scope reference for the given frame.
pub fn scope_locals_ref(frame_id: usize) -> u64 {
    (frame_id as u64) * 2 + 1
}

/// Encode an upvalues scope reference for the given frame.
pub fn scope_upvalues_ref(frame_id: usize) -> u64 {
    (frame_id as u64) * 2 + 2
}

/// Decode a scope variable reference into frame ID and kind.
pub fn decode_scope_ref(reference: u64) -> Option<ScopeKind> {
    if reference == 0 || reference >= DEBUG_VALUE_REF_BASE {
        return None;
    }
    if reference % 2 == 1 {
        Some(ScopeKind::Locals(((reference - 1) / 2) as usize))
    } else {
        Some(ScopeKind::Upvalues(((reference - 2) / 2) as usize))
    }
}

/// Events sent from the VM backend to the DAP frontend.
#[derive(Debug)]
pub enum DebugEvent {
    Stopped {
        reason: StopReason,
        description: Option<String>,
    },
    Terminated,
    Output {
        category: String,
        output: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum StopReason {
    Breakpoint,
    Step,
    Pause,
    Entry,
    Exception,
}

/// Mutable debugger state carried alongside the VM.
pub struct DebugState {
    /// Active breakpoints: (file_path, line) → breakpoint ID
    pub breakpoints: HashMap<(PathBuf, u32), u32>,
    /// Conditional breakpoints: (file_path, line) → condition expression. A
    /// breakpoint with a condition only fires when the expression evaluates
    /// truthy in the stopped frame. Keys are a subset of `breakpoints`.
    pub conditions: HashMap<(PathBuf, u32), String>,
    /// Whether to stop on uncaught runtime errors (set via setExceptionBreakpoints).
    pub break_on_uncaught: bool,
    /// Message of the last uncaught error we stopped on, for the exceptionInfo request.
    pub last_exception: Option<String>,
    /// Valid executable source lines for the currently debugged program, keyed by source file.
    pub valid_breakpoint_lines: BTreeMap<PathBuf, Vec<u32>>,
    /// Current step mode
    pub step_mode: StepMode,
    /// Frame depth when stepping was initiated
    pub step_frame_depth: usize,
    /// Last source location we stopped at (file, line)
    pub last_stop_line: Option<(PathBuf, u32)>,
    /// External pause request (set by DAP frontend, checked by VM)
    pub pause_requested: bool,
    /// Skip debug stop checks while on the same line as last_stop_line.
    /// Set after returning Stopped; cleared when execution moves to a different line.
    pub resume_skip: bool,
    /// Instruction budget for cooperative yielding. When > 0, the VM will yield
    /// after executing approximately this many instructions. Set to 0 to disable.
    /// Decremented during execution; caller should reset before each resume.
    pub instructions_remaining: u32,
    /// Channel to send events to the DAP frontend
    pub event_tx: mpsc::Sender<DebugEvent>,
    /// Channel to receive commands from the DAP frontend
    pub command_rx: mpsc::Receiver<DebugCommand>,
    /// True for a cooperative (WASM playground) session with no real command
    /// channel: stops are surfaced by RETURNING `VmExecResult::Stopped` from
    /// `run_cooperative`/`start_cooperative` and resumed by a later call, never
    /// by blocking on `command_rx`. The async scheduler consults this to decide
    /// whether a mid-task breakpoint blocks (`handle_debug_stop`, native DAP) or
    /// surfaces as a cooperative stop (this flag, WASM). Set by `new_headless`.
    headless: bool,
    next_bp_id: u32,
}

impl DebugState {
    pub fn new(
        event_tx: mpsc::Sender<DebugEvent>,
        command_rx: mpsc::Receiver<DebugCommand>,
    ) -> Self {
        DebugState {
            breakpoints: HashMap::new(),
            conditions: HashMap::new(),
            break_on_uncaught: false,
            last_exception: None,
            valid_breakpoint_lines: BTreeMap::new(),
            step_mode: StepMode::Continue,
            step_frame_depth: 0,
            last_stop_line: None,
            pause_requested: false,
            resume_skip: false,
            instructions_remaining: 0,
            event_tx,
            command_rx,
            headless: false,
            next_bp_id: 1,
        }
    }

    /// Create a DebugState without functional channels.
    /// Used for cooperative (WASM) execution where commands are applied
    /// between `run_inner` calls, not via channels.
    pub fn new_headless() -> Self {
        let (event_tx, _) = mpsc::channel();
        let (_, command_rx) = mpsc::channel();
        DebugState {
            breakpoints: HashMap::new(),
            conditions: HashMap::new(),
            break_on_uncaught: false,
            last_exception: None,
            valid_breakpoint_lines: BTreeMap::new(),
            step_mode: StepMode::Continue,
            step_frame_depth: 0,
            last_stop_line: None,
            pause_requested: false,
            resume_skip: false,
            instructions_remaining: 0,
            event_tx,
            command_rx,
            headless: true,
            next_bp_id: 1,
        }
    }

    /// Whether this is a cooperative (WASM) session with no real command
    /// channel. See [`DebugState::headless`].
    pub fn is_headless(&self) -> bool {
        self.headless
    }

    /// Check if we should stop at the given span and frame depth.
    pub fn should_stop(&self, file: Option<&PathBuf>, line: u32, frame_depth: usize) -> bool {
        if self.pause_requested {
            return true;
        }

        if let Some(f) = file {
            if self.breakpoints.contains_key(&(f.clone(), line)) {
                return true;
            }
        }

        match self.step_mode {
            StepMode::Continue => false,
            StepMode::StepInto => self.moved_since_last_stop(file, line),
            StepMode::StepOver => {
                frame_depth <= self.step_frame_depth && self.moved_since_last_stop(file, line)
            }
            StepMode::StepOut => frame_depth < self.step_frame_depth,
        }
    }

    /// Whether `(file, line)` differs from the last place a step stopped — the guard
    /// that keeps StepInto/StepOver from re-stopping on the SAME source location. Both
    /// the file AND line must match the prior stop to be considered "same"; comparing
    /// only the line would wrongly treat the same line number in a DIFFERENT file as no
    /// movement (silently stepping past the callee's first line).
    fn moved_since_last_stop(&self, file: Option<&PathBuf>, line: u32) -> bool {
        match &self.last_stop_line {
            Some((last_file, last_line)) => line != *last_line || file != Some(last_file),
            None => true,
        }
    }

    /// Whether a stop at `(file, line)` with the given frame depth is caused
    /// *solely* by a breakpoint hit — i.e. there is no pending pause and the
    /// step mode would not stop here on its own. Conditional breakpoints are
    /// only gated by their condition in this case; a stop that would also be a
    /// step/pause stop always fires regardless of any condition.
    pub fn is_pure_breakpoint_stop(
        &self,
        file: Option<&PathBuf>,
        line: u32,
        frame_depth: usize,
    ) -> bool {
        if self.pause_requested {
            return false;
        }
        let on_breakpoint = file.is_some_and(|f| self.breakpoints.contains_key(&(f.clone(), line)));
        if !on_breakpoint {
            return false;
        }
        let step_would_stop = match self.step_mode {
            StepMode::Continue => false,
            StepMode::StepInto => self.moved_since_last_stop(file, line),
            StepMode::StepOver => {
                frame_depth <= self.step_frame_depth && self.moved_since_last_stop(file, line)
            }
            StepMode::StepOut => frame_depth < self.step_frame_depth,
        };
        !step_would_stop
    }

    /// The condition expression for the breakpoint at `(file, line)`, if any.
    pub fn condition_at(&self, file: Option<&PathBuf>, line: u32) -> Option<&str> {
        let f = file?;
        self.conditions.get(&(f.clone(), line)).map(|s| s.as_str())
    }

    /// Set breakpoints for a file, replacing any existing ones for that file.
    ///
    /// Convenience wrapper for unconditional breakpoints (used by the WASM
    /// cooperative debugger). Conditional breakpoints go through
    /// [`set_breakpoints_with_conditions`].
    pub fn set_breakpoints(&mut self, file: &PathBuf, lines: &[u32]) -> Vec<DapBreakpoint> {
        let requested: Vec<SourceBreakpoint> = lines
            .iter()
            .map(|&line| SourceBreakpoint {
                line,
                condition: None,
            })
            .collect();
        self.set_breakpoints_with_conditions(file, &requested)
    }

    /// Set breakpoints (optionally conditional) for a file, replacing any
    /// existing ones for that file. A breakpoint whose `condition` is set only
    /// fires when the expression evaluates truthy in the stopped frame; the
    /// expression text is stored against the resolved (snapped) line.
    pub fn set_breakpoints_with_conditions(
        &mut self,
        file: &PathBuf,
        requested: &[SourceBreakpoint],
    ) -> Vec<DapBreakpoint> {
        let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.clone());
        self.breakpoints.retain(|(f, _), _| f != &file);
        self.conditions.retain(|(f, _), _| f != &file);

        let valid_lines = self.valid_breakpoint_lines.get(&file);
        requested
            .iter()
            .map(|bp| {
                let requested_line = bp.line;
                let resolved = match valid_lines {
                    Some(valid) => crate::vm::snap_breakpoint_line(requested_line, valid),
                    None => Some(requested_line),
                };
                match resolved {
                    Some(line) => {
                        let id = self.next_bp_id;
                        self.next_bp_id += 1;
                        self.breakpoints.insert((file.clone(), line), id);
                        if let Some(cond) = &bp.condition {
                            if !cond.trim().is_empty() {
                                self.conditions.insert((file.clone(), line), cond.clone());
                            }
                        }
                        DapBreakpoint {
                            id,
                            verified: true,
                            requested_line,
                            line,
                            message: (line != requested_line).then(|| {
                                format!("Breakpoint moved to nearest executable line {line}")
                            }),
                        }
                    }
                    None => DapBreakpoint {
                        id: 0,
                        verified: false,
                        requested_line,
                        line: requested_line,
                        message: Some("No executable line exists in this source".to_string()),
                    },
                }
            })
            .collect()
    }

    pub fn set_valid_breakpoint_lines(&mut self, lines: BTreeMap<PathBuf, Vec<u32>>) {
        self.valid_breakpoint_lines = lines;
    }
}

/// Result of cooperative VM execution.
#[derive(Debug)]
pub enum VmExecResult {
    /// Execution completed normally with a return value.
    Finished(Value),
    /// Execution paused at a debug stop point.
    Stopped(StopInfo),
    /// Execution yielded after exhausting the instruction budget.
    /// Call `run_cooperative` again to continue.
    Yielded,
    /// Execution stopped at an opcode boundary after consuming its instruction quantum.
    QuantumExpired { instructions: usize },
    /// Execution suspended for async yield (channel op, await, sleep).
    AsyncYield(sema_core::YieldReason),
}

/// Information about why and where the VM stopped.
#[derive(Debug, Clone)]
pub struct StopInfo {
    pub reason: StopReason,
    pub file: Option<PathBuf>,
    pub line: u32,
}
