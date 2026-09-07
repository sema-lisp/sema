use std::cell::RefCell;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use sema_core::{pretty_print, SemaError};
use sema_eval::Interpreter;

mod ast;
mod build;
mod colors;
mod completions;
mod cross_compile;
mod disasm;
mod docs;
mod eval_cli;
mod fmt;
mod import_tracer;
mod notebook_cli;
mod output;
mod pkg;
mod repl;
mod update;
mod web;
mod workflow_check;
mod workflow_cli;
// The dashboard server itself lives in the `sema` LIBRARY crate
// (`crates/sema/src/lib.rs` → `pub mod workflow_view;`), not here, so
// `crates/sema/tests/*.rs` integration tests can drive it in-process.
// `workflow_cli` uses it as `sema::workflow_view::…`.

/// Read a source file with consistent, friendly error messages.
///
/// Standardises the wording across all subcommands so users see the same
/// phrasing for not-found / permission-denied errors regardless of which
/// command they ran.
fn read_source_file(path: impl AsRef<Path>) -> Result<String, String> {
    let p = path.as_ref();
    std::fs::read_to_string(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("file not found: {}", p.display()),
        std::io::ErrorKind::PermissionDenied => {
            format!("permission denied: {}", p.display())
        }
        _ => format!("reading {}: {}", p.display(), e),
    })
}

thread_local! {
    /// The most recent program source (with the file it came from, if any) whose
    /// errors are still eligible for a source-snippet render. Set as a pair so
    /// the `file` and `source` can never disagree.
    pub(crate) static LAST_INPUT: RefCell<Option<(String, Option<PathBuf>)>> =
        const { RefCell::new(None) };
}

/// Record the current program source for error snippets. `file` is `Some` for
/// `--load`/positional-FILE paths and `None` for REPL/stdin/`-e` input.
pub(crate) fn set_last_input(source: &str, file: Option<PathBuf>) {
    LAST_INPUT.with(|s| *s.borrow_mut() = Some((source.to_string(), file)));
}

/// The most recently recorded `(source, file)` pair, if any.
pub(crate) fn last_input() -> Option<(String, Option<PathBuf>)> {
    LAST_INPUT.with(|s| s.borrow().clone())
}

// REPL completer, command set, and trait impls have moved to `src/repl/`.

#[derive(Parser)]
#[command(name = "sema", about = "Sema: A Lisp with LLM primitives", version)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// File to execute
    #[arg(conflicts_with_all = ["eval", "print"])]
    file: Option<String>,

    /// Evaluate an expression and print result (if non-nil)
    #[arg(short, long, conflicts_with_all = ["print", "file"])]
    eval: Option<String>,

    /// Evaluate an expression and always print result
    #[arg(short, long, conflicts_with_all = ["eval", "file"])]
    print: Option<String>,

    /// Load file(s) before executing
    #[arg(short, long = "load", action = clap::ArgAction::Append)]
    load: Vec<String>,

    /// Suppress REPL banner
    #[arg(short, long)]
    quiet: bool,

    /// Enter REPL after running file or eval
    #[arg(short, long)]
    interactive: bool,

    /// Disable LLM features (skip provider auto-configuration)
    #[arg(long)]
    no_llm: bool,

    /// Sandbox mode: restrict dangerous operations.
    /// Values: "strict", "all", or comma-separated list like "no-shell,no-network,no-fs-write"
    /// Available capabilities: shell, fs-read, fs-write, network, env-read, env-write, process, llm, serial
    #[arg(long)]
    sandbox: Option<String>,

    /// Set default chat model
    #[arg(long)]
    chat_model: Option<String>,

    /// Set chat provider (anthropic, openai, gemini, groq, xai, mistral, moonshot, ollama)
    #[arg(long)]
    chat_provider: Option<String>,

    /// Set embedding model
    #[arg(long)]
    embedding_model: Option<String>,

    /// Set embedding provider (jina, voyage, cohere, openai)
    #[arg(long)]
    embedding_provider: Option<String>,

    /// Restrict file operations to these directories (comma-separated)
    #[arg(long, value_name = "DIRS")]
    allowed_paths: Option<String>,

    /// Arguments passed to the script (after --)
    #[arg(last = true)]
    script_args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse source and print the AST
    Ast {
        /// File to parse
        file: Option<String>,

        /// Expression to parse
        #[arg(short, long)]
        eval: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,

        /// Install completions to the standard location
        #[arg(long)]
        install: bool,
    },
    /// Compile source to bytecode (.semac); imports resolve at runtime
    Compile {
        /// Source file to compile
        file: String,

        /// Output file path (default: input with .semac extension)
        #[arg(short, long)]
        output: Option<String>,

        /// Validate a .semac file without executing
        #[arg(long)]
        check: bool,
    },
    /// Disassemble a .semac bytecode file
    Disasm {
        /// Bytecode file to disassemble
        file: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Browse builtin and special-form docs
    #[command(args_conflicts_with_subcommands = true)]
    Doc {
        /// Show docs in a pager even when the output fits on one screen
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,

        /// Print directly without invoking a pager
        #[arg(long, conflicts_with = "pager")]
        no_pager: bool,

        #[command(subcommand)]
        command: Option<DocCommands>,

        /// Symbol to show documentation for (implicit `show`)
        symbol: Option<String>,
    },
    /// Sema package manager for adding, updating, and publishing dependencies
    Pkg {
        /// Output a machine-readable JSON result
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        command: PkgCommands,
    },
    /// Build a standalone executable with dependencies bundled
    Build {
        /// Source file to compile and bundle
        #[arg(required_unless_present = "list_targets")]
        file: Option<String>,

        /// Output executable path (default: filename without extension)
        #[arg(short, long)]
        output: Option<String>,

        /// Additional files or directories to bundle (repeatable)
        #[arg(long = "include", action = clap::ArgAction::Append)]
        includes: Vec<String>,

        /// Path to a sema executable to embed the program into, instead of this
        /// executable (the default) or the release binary that --target downloads.
        /// The output inherits its platform and version — pass e.g. a Windows
        /// sema.exe to cross-build without a download. Conflicts with --target.
        #[arg(long, value_name = "SEMA_BINARY", conflicts_with = "target")]
        runtime: Option<String>,

        /// Target platform triple or alias (e.g. linux, macos, windows, web, or a full triple).
        /// Use "all" to build for all supported targets.
        #[arg(long)]
        target: Option<String>,

        /// Show all supported target platforms
        #[arg(long)]
        list_targets: bool,

        /// Force re-download of cached runtime binaries
        #[arg(long)]
        no_cache: bool,

        /// Show per-step build detail and runtime cache/checksum info
        #[arg(short, long)]
        verbose: bool,

        /// Print a machine-readable build manifest to stdout (paths, sizes,
        /// sha256, per-target status)
        #[arg(long)]
        json: bool,
    },
    /// Format Sema source files
    Fmt {
        /// Files or glob patterns to format (default: **/*.sema in current directory)
        files: Vec<String>,

        /// Check formatting without writing changes (exit 1 if unformatted)
        #[arg(long)]
        check: bool,

        /// Print diff of formatting changes
        #[arg(long)]
        diff: bool,

        /// Max line width (default: 80, or value from sema.toml)
        #[arg(long)]
        width: Option<usize>,

        /// Indentation width for body forms (default: 2, or value from sema.toml)
        #[arg(long)]
        indent: Option<usize>,

        /// Align consecutive similar forms (defines, cond clauses, let bindings)
        #[arg(long)]
        align: bool,

        /// Max consecutive blank lines to keep (default: 1, or value from sema.toml)
        #[arg(long)]
        max_blank_lines: Option<usize>,

        /// Emit read-only NDJSON results for editor integrations
        #[arg(long, conflicts_with = "diff")]
        json: bool,
    },
    /// Start the Language Server Protocol (LSP) server
    Lsp,
    /// Start the Debug Adapter Protocol (DAP) server
    Dap,
    /// Start the Model Context Protocol (MCP) server, or manage client auth (`login`/`logout`/`list`)
    #[command(args_conflicts_with_subcommands = true)]
    Mcp {
        /// Client-auth subcommand; when omitted, runs the MCP server
        #[command(subcommand)]
        auth: Option<McpAuthCommands>,
        /// Optional source files to run/load tools from (server mode)
        #[arg(value_name = "FILES")]
        files: Vec<String>,
        /// Comma-separated list of tool names to explicitly include
        #[arg(long, value_name = "TOOLS")]
        include: Option<String>,
        /// Comma-separated list of tool names to explicitly exclude
        #[arg(long, value_name = "TOOLS")]
        exclude: Option<String>,
        /// Sandbox mode for the server (e.g. "strict" or "no-fs-write,no-shell").
        /// Defaults to the top-level --sandbox value, else allows everything.
        #[arg(long, value_name = "MODE")]
        sandbox: Option<String>,
        /// Default per-tool-call timeout in milliseconds for eval/run_file/
        /// tool-handler calls (a caller can override it per call via a
        /// `timeout_ms` argument). 0 disables the timeout: a runaway loop
        /// then wedges the single-threaded server indefinitely (issue #153),
        /// so this is not recommended. Defaults to SEMA_MCP_EVAL_TIMEOUT_MS
        /// if set, else 300000 (5 minutes).
        #[arg(long, value_name = "MS")]
        timeout_ms: Option<u64>,
    },
    /// Cell-based notebook with a browser UI
    Notebook {
        #[command(subcommand)]
        command: NotebookCommands,
    },
    /// Serve a sema-web app in the browser with a native LLM proxy
    Web {
        /// Path to the app's entry `.sema` file
        file: String,
        /// Host to bind. Loopback by default; a non-loopback host exposes the
        /// unauthenticated LLM proxy to the network.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on (advances to the next free port if taken)
        #[arg(short, long, default_value = "3000")]
        port: u16,
        /// Don't open a browser automatically
        #[arg(long)]
        no_open: bool,
        /// Disable the built-in LLM proxy
        #[arg(long)]
        no_llm: bool,
    },
    /// Run journaled workflows and view their runs
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommands,
    },
    /// Evaluate an expression or program and print the result
    Eval {
        /// Read program from stdin instead of --expr
        #[arg(long)]
        stdin: bool,

        /// Expression to evaluate (alternative to --stdin)
        #[arg(long)]
        expr: Option<String>,

        /// Emit machine-readable JSON result envelope
        #[arg(long)]
        json: bool,

        /// Set file path for error spans and relative import resolution
        #[arg(long)]
        path: Option<String>,

        /// Kill evaluation after N milliseconds; 0 disables the timeout (default: 5000)
        #[arg(long, default_value = "5000")]
        timeout: u64,

        /// Sandbox mode (e.g., "strict", "all", or comma-separated capabilities)
        #[arg(long)]
        sandbox: Option<String>,

        /// Disable LLM features
        #[arg(long)]
        no_llm: bool,
    },
    /// Update sema itself to the latest released version
    Update {
        /// Check for an available update without installing it
        #[arg(long)]
        check: bool,

        /// Install a specific version instead of the latest (e.g. "1.30.0")
        #[arg(long)]
        version: Option<String>,

        /// Skip the confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DocCommands {
    /// Show documentation for a symbol
    Show {
        /// Symbol to show documentation for
        symbol: String,
    },
    /// Search documentation by natural-language query
    Search {
        /// Query to search for
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,

        /// Maximum number of results to show
        #[arg(short = 'n', long, default_value_t = sema_mcp::docs_search::DEFAULT_LIMIT)]
        limit: usize,
    },
    /// Search symbol names by prefix, substring, and fuzzy match
    Apropos {
        /// Pattern to search for
        pattern: String,
    },
}

#[derive(Subcommand)]
enum McpAuthCommands {
    /// Log in to a remote (HTTP) MCP server and cache the OAuth token
    Login {
        /// The MCP server URL (e.g. https://mcp.example.com/mcp)
        url: String,
        /// Use the device-authorization flow instead of opening a browser
        #[arg(long, conflicts_with = "token")]
        device: bool,
        /// A pre-registered OAuth client id (when the server has no dynamic registration)
        #[arg(long = "client-id", value_name = "ID")]
        client_id: Option<String>,
        /// Store a pre-issued access token directly, skipping discovery/DCR/OAuth
        /// entirely — the headless/CI escape hatch (no browser, no device flow).
        #[arg(long, conflicts_with = "device", value_name = "TOKEN")]
        token: Option<String>,
        /// Seconds until the pre-issued --token expires (omit for a non-expiring token)
        #[arg(long, requires = "token", value_name = "SECS")]
        expires_in: Option<u64>,
    },
    /// Remove cached credentials for a remote MCP server
    Logout {
        /// The MCP server URL whose cached credentials to clear
        url: String,
    },
    /// List servers with cached credentials and each token's status (local only)
    List,
}

#[derive(Subcommand)]
enum PkgCommands {
    /// Add a package (from registry or git URL)
    Add {
        /// Package name or URL, optionally with @version (e.g., http-helpers@1.0.0 or github.com/user/repo@v1.0)
        spec: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Install all dependencies from sema.toml
    Install {
        /// Fail if sema.lock is missing or out of sync (for CI)
        #[arg(long)]
        locked: bool,
    },
    /// Update a package (or all packages)
    Update {
        /// Package name to update (updates all if omitted)
        name: Option<String>,
    },
    /// Remove an installed package
    Remove {
        /// Package URL or name
        name: String,
    },
    /// List installed packages
    List,
    /// Initialize a new sema.toml in the current directory
    Init,
    /// Authenticate with a package registry
    Login {
        /// API token (from registry account page)
        #[arg(long, conflicts_with = "username")]
        token: Option<String>,

        /// Registry username — exchanges the password for a fresh API token
        #[arg(long)]
        username: Option<String>,

        /// Registry password (prompted when --username is given without it)
        #[arg(long, requires = "username")]
        password: Option<String>,

        /// Registry URL (default: https://pkg.sema-lang.com)
        #[arg(long, default_value = "https://pkg.sema-lang.com")]
        registry: String,
    },
    /// Remove stored registry credentials
    Logout,
    /// Show which registry account the stored token belongs to
    Whoami {
        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// View or set package manager configuration
    Config {
        /// Config key (e.g., registry.url). Omit to show all config
        key: Option<String>,

        /// Value to set. Omit to read the current value
        value: Option<String>,
    },
    /// Publish current package to the registry
    Publish {
        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Search the registry for packages
    Search {
        /// Search query
        query: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Yank a published version (prevent new installs)
    Yank {
        /// Package@version to yank (e.g., my-package@0.1.0)
        spec: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Show package info from the registry
    Info {
        /// Package name
        name: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ApprovalMode {
    /// Prompt on a real terminal unless a durable authority is configured;
    /// otherwise pause with exit code 3.
    Auto,
    /// Require an interactive terminal prompt.
    Prompt,
    /// Leave the request pending and exit 3.
    Pause,
    /// Do not prompt; fail the command when a gate is reached.
    Deny,
}

fn resolve_approval_mode(
    requested: ApprovalMode,
    interactive: bool,
    has_durable_authority: bool,
) -> ApprovalMode {
    match requested {
        ApprovalMode::Auto if has_durable_authority => ApprovalMode::Pause,
        ApprovalMode::Auto if interactive => ApprovalMode::Prompt,
        ApprovalMode::Auto => ApprovalMode::Pause,
        mode => mode,
    }
}

fn validate_interactive_approval_authority(
    mode: ApprovalMode,
    interactive: bool,
    has_public_key: bool,
    has_signing_key: bool,
) -> Result<(), &'static str> {
    if mode == ApprovalMode::Prompt && interactive && has_public_key && !has_signing_key {
        Err(
            "--approval-mode prompt cannot sign with --approval-public-key-file; omit the public key for an ephemeral terminal authority, or use --view with the matching --approval-signing-key-file",
        )
    } else {
        Ok(())
    }
}

#[derive(Subcommand)]
enum WorkflowCommands {
    /// Run a workflow file (a `.sema` program that `defworkflow`s and runs it),
    /// journaling a frozen run-directory and writing `result.json`.
    ///
    /// Exit codes: 0 success, 1 failed/rejected, 2 needs MCP authentication, 3
    /// needs human approval. On an interactive terminal the default mode handles
    /// auth and approval inline; the flags below select headless behavior.
    Run {
        /// Path to the `.sema` workflow file.
        file: String,

        /// JSON object bound to the global `*workflow-args*` for the run.
        #[arg(long, default_value = "{}")]
        args: String,

        /// Base directory for run journals; the run lands in `<run-dir>/<run-id>/`.
        /// Defaults to the project-local `.sema/runs`.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Also start the live web viewer and keep it open after the run, so you
        /// can watch the run progress and inspect it afterwards.
        #[arg(long)]
        view: bool,

        /// Port for the `--view` viewer.
        #[arg(short, long, default_value = "8899")]
        port: u16,

        /// Resume a prior run by its run-id: reuse `<run-dir>/<run-id>/`, skip leaves
        /// already recorded in its `memo/` dir (no re-call of the model), and write a
        /// fresh `events.resume-N.jsonl` segment. A workflow edit changes the code
        /// version and re-runs everything.
        #[arg(long)]
        resume: Option<String>,

        /// Never log in inline on a needs-auth gate, even on an interactive
        /// terminal — always exit 2 with `sema mcp login` guidance instead. No
        /// effect when running headlessly (no TTY, or `CI` set): that already
        /// gets the exit-2 behavior.
        #[arg(long)]
        no_auth_prompt: bool,

        /// How a pending human approval is handled.
        #[arg(long, value_enum, default_value = "auto")]
        approval_mode: ApprovalMode,

        /// File containing the base64 Ed25519 public key authorized to decide headless
        /// approval requests. Interactive prompt mode generates an in-memory key when
        /// this is omitted.
        #[arg(long)]
        approval_public_key_file: Option<String>,

        /// Private approval authority exposed only to the loopback web viewer. Its
        /// public key becomes the run authority; the private key never enters Sema.
        #[arg(long, requires = "view")]
        approval_signing_key_file: Option<String>,

        /// Default actor recorded for decisions made in the web viewer.
        #[arg(long, requires = "approval_signing_key_file")]
        approval_actor: Option<String>,
    },
    /// List durable approval requests and decisions for one run.
    Approvals {
        /// Run id (the single directory name under --run-dir).
        run_id: String,

        /// Base directory holding workflow run directories.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Emit a JSON array instead of a human-readable list.
        #[arg(long)]
        json: bool,
    },
    /// Approve one pending workflow request.
    Approve {
        run_id: String,
        approval_id: String,

        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Optional audit comment.
        #[arg(long)]
        comment: Option<String>,

        /// Actor recorded in the decision (defaults to SEMA_APPROVAL_ACTOR/USER).
        #[arg(long)]
        actor: Option<String>,

        /// Private Ed25519 PKCS#8 key created by `workflow approval-keygen`.
        #[arg(long)]
        signing_key_file: String,
    },
    /// Reject one pending workflow request.
    Reject {
        run_id: String,
        approval_id: String,

        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Required rejection reason recorded in the decision.
        #[arg(long)]
        reason: String,

        /// Actor recorded in the decision (defaults to SEMA_APPROVAL_ACTOR/USER).
        #[arg(long)]
        actor: Option<String>,

        /// Private Ed25519 PKCS#8 key created by `workflow approval-keygen`.
        #[arg(long)]
        signing_key_file: String,
    },
    /// Generate an Ed25519 approval authority key pair.
    ApprovalKeygen {
        /// New private-key file (created with mode 0600 on Unix).
        #[arg(long)]
        private_key_file: String,

        /// New public-key file safe to pass to workflow runs.
        #[arg(long)]
        public_key_file: String,
    },
    /// Backfill the cross-run SQLite index (`<run-dir>/index.db`) from every run's
    /// journal — for offline/CI use; the viewer also syncs lazily on request.
    Index {
        /// Base directory holding `<run-id>/events.jsonl` run journals.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,
    },
    /// Export a deterministic evidence bundle for one completed workflow run.
    Export {
        /// Run id (the single directory name under `--run-dir`).
        run_id: String,

        /// Base directory holding `<run-id>/events.jsonl` run journals.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Output directory. Defaults to `<run-dir>/<run-id>/evidence`.
        #[arg(long)]
        out_dir: Option<String>,
    },
    /// Open the web viewer for a run directory's workflow journals
    View {
        /// Base directory holding `<run-id>/events.jsonl` run journals.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Host to bind. Defaults to loopback; binding elsewhere exposes the run
        /// directory to the network (the viewer has no auth).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to listen on.
        #[arg(short, long, default_value = "8899")]
        port: u16,

        /// Private Ed25519 key enabling approve/reject controls. Approval-enabled
        /// viewers are restricted to loopback hosts.
        #[arg(long)]
        approval_signing_key_file: Option<String>,

        /// Default actor recorded for decisions made in the web viewer.
        #[arg(long, requires = "approval_signing_key_file")]
        approval_actor: Option<String>,
    },
    /// Statically validate a workflow `.sema` file WITHOUT evaluating it or calling any LLM
    /// — catches arity traps, bad step opts, and layout issues before a run.
    Check {
        /// Path to the `.sema` workflow file.
        file: String,

        /// Treat warnings as errors (exit non-zero if any warning fires).
        #[arg(long)]
        strict: bool,

        /// Emit machine-readable JSON diagnostics instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
#[command(subcommand_required = true, arg_required_else_help = true)]
enum NotebookCommands {
    /// Start the notebook server with browser UI
    Serve {
        /// Path to .sema-nb file (created if absent)
        file: Option<String>,

        /// Host address to bind to. Defaults to loopback (127.0.0.1); the
        /// notebook server has no auth layer, so binding to a non-loopback
        /// address exposes unauthenticated code execution to the network.
        #[arg(long, default_value = sema_notebook::server::DEFAULT_HOST)]
        host: String,

        /// Port to listen on
        #[arg(short, long, default_value = "8888")]
        port: u16,
    },
    /// Run all cells in a notebook headlessly
    Run {
        /// Path to .sema-nb file
        file: String,

        /// Only run specific cells (1-based, comma-separated)
        #[arg(long)]
        cells: Option<String>,
    },
    /// Export a notebook to Markdown
    Export {
        /// Path to .sema-nb file
        file: String,

        /// Output format
        #[arg(long, default_value = "md")]
        format: String,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Create a new empty notebook
    New {
        /// Path to create the .sema-nb file
        file: String,

        /// Notebook title
        #[arg(short, long)]
        title: Option<String>,
    },
}

/// Default `sema mcp` per-tool-call timeout: a wedge-recovery backstop
/// (issue #153), not a fine-grained UX limit — generous enough that a
/// legitimate `llm/*` agent-loop eval isn't cut off, since a CPU-bound
/// runaway loop is caught almost immediately regardless of how long the
/// deadline is (`EvalContext::check_loop_interrupt` polls it every ~16k VM
/// steps).
const DEFAULT_MCP_TOOL_TIMEOUT_MS: u64 = 300_000;

/// Resolve the `sema mcp` per-tool-call timeout: an explicit `--timeout-ms`
/// wins outright; otherwise `SEMA_MCP_EVAL_TIMEOUT_MS` (same precedence and
/// env var naming convention as `sema-notebook`'s `resolve_cell_timeout`);
/// otherwise `DEFAULT_MCP_TOOL_TIMEOUT_MS`. `0` (from either source) means
/// "disabled" and returns `None`.
fn resolve_mcp_tool_timeout(explicit_ms: Option<u64>) -> Option<std::time::Duration> {
    let ms = explicit_ms.unwrap_or_else(|| {
        std::env::var("SEMA_MCP_EVAL_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_MCP_TOOL_TIMEOUT_MS)
    });
    if ms == 0 {
        None
    } else {
        Some(std::time::Duration::from_millis(ms))
    }
}

/// `sema mcp`: the auth subcommands, or the stdio MCP server. `sandbox` is the
/// top-level `--sandbox` selection; `mcp_sandbox` (the subcommand flag) wins.
fn run_mcp(
    auth: Option<McpAuthCommands>,
    files: Vec<String>,
    include: Option<String>,
    exclude: Option<String>,
    mcp_sandbox: Option<String>,
    timeout_ms: Option<u64>,
    sandbox: sema_core::Sandbox,
) {
    if let Some(auth) = auth {
        let result = match auth {
            McpAuthCommands::Login {
                url,
                token: Some(token),
                expires_in,
                ..
            } => sema_mcp::mcp_login_token(&url, &token, expires_in),
            McpAuthCommands::Login {
                url,
                device,
                client_id,
                token: None,
                ..
            } => sema_mcp::mcp_login(&url, device, client_id.as_deref()),
            McpAuthCommands::Logout { url } => sema_mcp::mcp_logout(&url),
            McpAuthCommands::List => sema_mcp::mcp_list(),
        };
        if let Err(e) = result {
            die(format!("MCP command failed: {e}"));
        }
        return;
    }
    let inc_tools = include.map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .collect::<Vec<String>>()
    });
    let exc_tools = exclude.map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .collect::<Vec<String>>()
    });

    // --sandbox on the subcommand wins; else the top-level --sandbox
    // (already parsed into `sandbox`, defaulting to allow-all). The
    // server historically hardcoded allow_all, so absent flags keep
    // exactly that behavior.
    let sandbox = match mcp_sandbox.as_deref() {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            die(format!("invalid MCP --sandbox value: {e}"));
        }),
        None => sandbox,
    };
    let interpreter = build_interpreter(&sandbox);

    let _ = interpreter.eval_str("(llm/auto-configure)");

    for file in files {
        match read_source_file(&file) {
            Ok(content) => {
                if let Err(e) = interpreter.eval_str_compiled(&content) {
                    die(format!("could not load tool file {file}: {e}"));
                }
            }
            Err(e) => {
                die(format!("could not read tool file {file}: {e}"));
            }
        }
    }

    // Sync loop, deliberately NO tokio runtime: with an ambient
    // runtime, llm/* builtins hit io_block_on's runtime-in-runtime
    // panic and killed the server on the first LLM tool call.
    let tool_timeout = resolve_mcp_tool_timeout(timeout_ms);
    if let Err(e) = sema_mcp::run_mcp_server_sync(interpreter, inc_tools, exc_tools, tool_timeout) {
        die(format!("MCP server failed: {e}"));
    }
}

/// Build the standard CLI interpreter: stdlib + LLM (registered inside sema-eval)
/// plus the MCP *client* builtins (`mcp/connect`, `mcp/tools`, `mcp/tools->sema`,
/// …). The MCP builtins live in `sema-mcp`, which depends on `sema-eval`, so they
/// can't be registered inside `sema-eval` itself — the binary wires them in here.
/// The real `WorkflowMcpResolver` (`sema::workflow_mcp`) is registered right
/// alongside them, so every CLI path built through this function (REPL, `sema
/// run`, `sema workflow run`, …) can resolve a workflow's declared `:mcp`
/// servers — see docs/plans/2026-06-24-workflow-mcp-auth.md §3/§9(a).
/// The sandbox selected by the top-level `--sandbox` / `--allowed-paths` flags;
/// `allow_all` when neither is given. An invalid spec exits with status 1.
fn sandbox_from_cli(spec: Option<&str>, allowed_paths: Option<&str>) -> sema_core::Sandbox {
    let sandbox = match spec {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| die(e)),
        None => sema_core::Sandbox::allow_all(),
    };
    match allowed_paths {
        Some(value) => sandbox.with_allowed_paths(sema_core::Sandbox::parse_allowed_paths(value)),
        None => sandbox,
    }
}

/// Run one stdio protocol server (LSP, DAP) to completion on a multi-thread
/// tokio runtime.
fn block_on_server<F: std::future::Future>(server: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime")
        .block_on(server)
}

fn build_interpreter(sandbox: &sema_core::Sandbox) -> Interpreter {
    let interpreter = Interpreter::new_with_sandbox(sandbox);
    sema_mcp::register_mcp_builtins(&interpreter.global_env, sandbox);
    sema::workflow_mcp::register_real_resolver();
    install_ctrlc_handler(&interpreter);
    interpreter
}

/// Install a process-wide Ctrl-C handler that cancels every live root on this
/// interpreter's persistent runtime, replacing the native CLI's former
/// reliance on `check_interrupt` polling (that TLS stays for wasm's SAB-cancel
/// path — see `crates/sema-core/src/async_signal.rs` — but nothing on native
/// installs the callback that would make it fire, so a native script had no
/// interruption path at all before this).
///
/// `ctrlc::set_handler` runs the closure on its own dedicated thread (not the
/// raw OS signal handler itself), so it needs no async-signal-safety, but it
/// DOES need to be `Send` — the closure below captures only
/// [`RuntimeCommandHandle`](sema_vm::runtime::RuntimeCommandHandle) (a
/// channel handle, `Send + Sync`, holding no `Rc`/`Value`/`Env`) plus plain
/// `Send` bookkeeping (an `Instant` and an `AtomicU64`), never the
/// `Interpreter` itself.
///
/// First Ctrl-C requests a graceful cancel (`cancel_all`): every live root
/// settles `Cancelled(HostStop)`, which the existing drive loop
/// (`Interpreter::drive_handle_to_settlement`) already surfaces as a normal
/// `SemaError` through whichever `eval_str*` call is driving the program, so
/// callers see the CLI's ordinary error-exit path with no new branch needed.
/// A second Ctrl-C within 2s of the first — the runtime is unresponsive, or a
/// Sema program is deliberately swallowing the cancellation — hard-exits
/// immediately (`128 + SIGINT`), the conventional double-interrupt escape
/// hatch. Ctrl-C presses spaced further apart than that are treated as
/// independent requests (e.g. a REPL session cancelling one long-running
/// command, then later cancelling an unrelated one) rather than accumulating
/// toward a hard exit.
///
/// Only meaningful while raw terminal mode is OFF (a plain blocking eval —
/// `-e`, a script file, or code running between REPL prompts): reedline only
/// enables raw mode for the duration of `read_line` and disables `ISIG`, so a
/// Ctrl-C keypress at the REPL prompt itself never reaches this handler as a
/// real `SIGINT` — it is delivered to reedline as a raw key event and handled
/// entirely by the REPL's own `Signal::CtrlC` branch, unchanged.
struct CliCtrlCState {
    handle: std::sync::Mutex<Option<sema_vm::runtime::RuntimeCommandHandle>>,
    prompt_active: std::sync::atomic::AtomicBool,
    started: std::time::Instant,
    last_sigint_ms: std::sync::atomic::AtomicU64,
}

static CLI_CTRLC_STATE: std::sync::OnceLock<CliCtrlCState> = std::sync::OnceLock::new();
static CLI_CTRLC_HANDLER: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

fn install_ctrlc_handler(interpreter: &Interpreter) {
    let state = CLI_CTRLC_STATE.get_or_init(|| CliCtrlCState {
        handle: std::sync::Mutex::new(None),
        prompt_active: std::sync::atomic::AtomicBool::new(false),
        started: std::time::Instant::now(),
        last_sigint_ms: std::sync::atomic::AtomicU64::new(0),
    });
    *state
        .handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(interpreter.command_handle());
    let result = CLI_CTRLC_HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| {
            let Some(state) = CLI_CTRLC_STATE.get() else {
                return;
            };
            // `read_line` may be restarted by the OS. Exiting is the only portable way
            // to make a prompt Ctrl-C immediate; the request was already durably
            // published, so this deliberately leaves it pending.
            if state
                .prompt_active
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                std::process::exit(130);
            }
            let now_ms = state.started.elapsed().as_millis() as u64;
            let previous_ms = state
                .last_sigint_ms
                .swap(now_ms, std::sync::atomic::Ordering::SeqCst);
            if is_double_interrupt(previous_ms, now_ms) {
                std::process::exit(130);
            }
            if let Some(handle) = state
                .handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
            {
                handle.cancel_all();
            }
        })
        .map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        print_cli_warning(format!("cannot install Ctrl-C handler: {error}"));
    }
}

struct ApprovalPromptCtrlCGuard;

impl ApprovalPromptCtrlCGuard {
    fn enter() -> Self {
        if let Some(state) = CLI_CTRLC_STATE.get() {
            state
                .prompt_active
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Self
    }
}

impl Drop for ApprovalPromptCtrlCGuard {
    fn drop(&mut self) {
        if let Some(state) = CLI_CTRLC_STATE.get() {
            state
                .prompt_active
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// The double-interrupt decision `install_ctrlc_handler` applies on every
/// `SIGINT`: `previous_ms`/`now_ms` are millis-since-process-start of the
/// prior and current signal (`0` for "no prior signal yet"). `true` means
/// hard-exit instead of another graceful `cancel_all` — the second Ctrl-C
/// arrived within `DOUBLE_INTERRUPT_WINDOW_MS` of the first, the
/// conventional "it's not responding, just kill it" escape hatch. Pulled out
/// of the signal-handler closure as a pure function so the window arithmetic
/// (in particular the `previous_ms != 0` "no prior signal" guard and the
/// `saturating_sub` protecting against a first signal landing at exactly
/// `0ms`) is unit-testable without spawning a process or sending real
/// signals.
fn is_double_interrupt(previous_ms: u64, now_ms: u64) -> bool {
    const DOUBLE_INTERRUPT_WINDOW_MS: u64 = 2_000;
    previous_ms != 0 && now_ms.saturating_sub(previous_ms) < DOUBLE_INTERRUPT_WINDOW_MS
}

#[cfg(test)]
mod ctrlc_tests {
    use super::workflow_cli::{format_needs_approval_guidance, shell_quote, terminal_safe};
    use super::{
        is_double_interrupt, resolve_approval_mode, validate_interactive_approval_authority,
        ApprovalMode,
    };
    use sema_core::Value;
    use std::collections::BTreeMap;

    #[test]
    fn first_signal_never_hard_exits() {
        // previous_ms == 0 is the sentinel for "no prior signal", including
        // the edge case of a first signal landing at exactly process-start.
        assert!(!is_double_interrupt(0, 0));
        assert!(!is_double_interrupt(0, 5_000));
    }

    #[test]
    fn second_signal_within_window_hard_exits() {
        assert!(is_double_interrupt(1_000, 1_001));
        assert!(is_double_interrupt(1_000, 2_999));
    }

    #[test]
    fn second_signal_outside_window_is_treated_as_independent() {
        assert!(!is_double_interrupt(1_000, 3_000));
        assert!(!is_double_interrupt(1_000, 60_000));
    }

    #[test]
    fn auto_approval_preserves_a_configured_durable_authority() {
        assert_eq!(
            resolve_approval_mode(ApprovalMode::Auto, true, true),
            ApprovalMode::Pause
        );
        assert_eq!(
            resolve_approval_mode(ApprovalMode::Auto, true, false),
            ApprovalMode::Prompt
        );
        assert_eq!(
            resolve_approval_mode(ApprovalMode::Auto, false, false),
            ApprovalMode::Pause
        );
    }

    #[test]
    fn interactive_prompt_rejects_a_public_only_authority() {
        assert!(
            validate_interactive_approval_authority(ApprovalMode::Prompt, true, true, false)
                .is_err()
        );
        assert!(
            validate_interactive_approval_authority(ApprovalMode::Prompt, true, true, true).is_ok()
        );
        assert!(
            validate_interactive_approval_authority(ApprovalMode::Prompt, false, true, false)
                .is_ok()
        );
    }

    #[test]
    fn approval_terminal_text_escapes_control_and_bidi_characters() {
        let rendered = terminal_safe("ok\n\u{1b}[31mspoof\u{061c}\u{202e}");
        assert_eq!(rendered, "ok\\u{a}\\u{1b}[31mspoof\\u{61c}\\u{202e}");
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\n'));

        #[cfg(not(windows))]
        {
            let quoted = shell_quote("line\n\u{202e}tail");
            assert_eq!(quoted, "$'line\\x0a\\u202etail'");
            assert!(!quoted.contains('\n'));
            assert!(!quoted.contains('\u{202e}'));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn approval_guidance_is_copy_safe_and_preserves_exact_resume_inputs() {
        let mut envelope = BTreeMap::new();
        envelope.insert(Value::keyword("run-id"), Value::string("run one"));
        envelope.insert(Value::keyword("approval-id"), Value::string("apr one"));
        let rendered = format_needs_approval_guidance(
            &Value::map(envelope),
            "/tmp/run dir",
            "/tmp/work flow.sema",
            r#"{"target":"a b"}"#,
            Some("/tmp/key file.public"),
            None,
        );
        assert!(rendered.contains("--signing-key-file \"$SEMA_APPROVAL_PRIVATE_KEY\""));
        assert!(!rendered.contains("<private-key-file>"));
        assert!(rendered.contains(
            r#"sema workflow run $'/tmp/work flow.sema' --args $'{"target":"a b"}' --resume $'run one' --run-dir $'/tmp/run dir' --approval-public-key-file $'/tmp/key file.public'"#
        ));
    }
}

fn main() {
    // reqwest is built with rustls-no-provider (see workspace Cargo.toml); install
    // the ring provider before anything (OTLP exporter, pkg, LLM) builds a client.
    sema_llm::http::ensure_crypto_provider();

    // Check for embedded archive before parsing CLI args
    if let Some(exit_code) = build::try_run_embedded() {
        std::process::exit(exit_code);
    }

    // Shell-completion helper for `sema doc` symbols, handled before clap parses.
    // It is intentionally NOT a clap subcommand: a hidden subcommand makes
    // `clap_complete`'s bash generator panic (find_subcommand_with_path), which
    // would break `sema completions bash`. The generated completion scripts still
    // invoke `sema __complete-doc-symbols <prefix>`.
    {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some("__complete-doc-symbols") {
            let prefix = args.next().unwrap_or_default();
            for name in docs::completion_candidates(&prefix) {
                println!("{name}");
            }
            return;
        }
    }

    let cli = Cli::parse();

    // Opt-in OpenTelemetry: installs a provider only when SEMA_OTEL_FILE or an OTLP
    // endpoint is configured (zero-cost no-op otherwise). Held for the process
    // lifetime; its Drop does the bounded flush+shutdown on normal return. (The JSONL
    // file exporter writes synchronously, so it survives a `std::process::exit` too.)
    let _otel_guard = sema_otel::init_from_env();

    let sandbox = sandbox_from_cli(cli.sandbox.as_deref(), cli.allowed_paths.as_deref());

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::Ast { file, eval, json } => {
                ast::run_ast(file, eval, json);
            }
            Commands::Completions { shell, install } => {
                if install {
                    completions::install_completions(shell);
                } else {
                    print!("{}", completions::generate_completions(shell));
                }
            }
            Commands::Compile {
                file,
                output,
                check,
            } => {
                if check {
                    build::run_check(&file);
                } else {
                    build::run_compile(&file, output.as_deref());
                }
            }
            Commands::Disasm { file, json } => {
                disasm::run_disasm(&file, json);
            }
            Commands::Doc {
                pager,
                no_pager,
                command,
                symbol,
            } => {
                let pager = if no_pager {
                    docs::PagerMode::Never
                } else if pager {
                    docs::PagerMode::Always
                } else {
                    docs::PagerMode::Auto
                };
                if let Err(msg) = run_doc(command, symbol, pager) {
                    die(msg);
                }
            }
            Commands::Pkg { json, command } => {
                if let Err(e) = pkg::run(command, json) {
                    die(e);
                }
            }
            Commands::Build {
                file,
                output,
                includes,
                runtime,
                target,
                list_targets,
                no_cache,
                verbose,
                json,
            } => {
                if list_targets {
                    cross_compile::list_targets();
                    return;
                }
                let file = file.expect("file is required unless --list-targets");
                if let Err(e) = build::run_build(
                    &file,
                    output.as_deref(),
                    &includes,
                    runtime.as_deref(),
                    target.as_deref(),
                    no_cache,
                    build::BuildOutputOpts { verbose, json },
                ) {
                    die(e);
                }
            }
            Commands::Fmt {
                files,
                check,
                diff,
                width,
                indent,
                align,
                max_blank_lines,
                json,
            } => {
                let config = fmt::find_config().unwrap_or_default();
                let opts = sema_fmt::FormatOptions {
                    width: width.unwrap_or(config.fmt.width),
                    indent: indent.unwrap_or(config.fmt.indent),
                    align: align || config.fmt.align,
                    max_blank_lines: max_blank_lines.unwrap_or(config.fmt.max_blank_lines),
                };
                fmt::run_fmt(&files, check, diff, &opts, &config.fmt.ignore, json);
            }
            Commands::Lsp => {
                eprintln!("Sema LSP server starting on stdio...");
                block_on_server(sema_lsp::run_server());
            }
            Commands::Dap => block_on_server(sema_dap::run_server()),
            Commands::Mcp {
                auth,
                files,
                include,
                exclude,
                sandbox: mcp_sandbox,
                timeout_ms,
            } => run_mcp(
                auth,
                files,
                include,
                exclude,
                mcp_sandbox,
                timeout_ms,
                sandbox,
            ),
            Commands::Notebook { command } => {
                notebook_cli::run_notebook_command(command);
            }
            Commands::Web {
                file,
                host,
                port,
                no_open,
                no_llm,
            } => {
                if let Err(e) = web::run(&file, &host, port, !no_open, !no_llm) {
                    die(format!("sema web failed: {e}"));
                }
            }
            Commands::Workflow { command } => {
                workflow_cli::run_workflow_command(command, &sandbox);
            }
            Commands::Eval {
                stdin,
                expr,
                json,
                path,
                timeout,
                sandbox,
                no_llm,
            } => {
                eval_cli::run_eval(stdin, expr, json, path, timeout, sandbox, no_llm);
            }
            Commands::Update {
                check,
                version,
                yes,
            } => {
                let opts = update::UpdateOptions {
                    check_only: check,
                    target_version: version,
                    yes,
                };
                if let Err(e) = update::run(opts) {
                    die(e);
                }
            }
        }
        return;
    }

    let interpreter = build_interpreter(&sandbox);

    // Set LLM env vars before auto-configure
    if let Some(model) = cli.chat_model.as_ref() {
        std::env::set_var("SEMA_CHAT_MODEL", model);
    }
    if let Some(provider) = cli.chat_provider.as_ref() {
        std::env::set_var("SEMA_CHAT_PROVIDER", provider);
    }
    if let Some(model) = &cli.embedding_model {
        std::env::set_var("SEMA_EMBEDDING_MODEL", model);
    }
    if let Some(provider) = &cli.embedding_provider {
        std::env::set_var("SEMA_EMBEDDING_PROVIDER", provider);
    }

    // Auto-configure LLM unless --no-llm
    if !cli.no_llm {
        if let Err(e) = interpreter.eval_str("(llm/auto-configure)") {
            if cli.chat_provider.is_some() || cli.chat_model.is_some() {
                print_error(&e);
                std::process::exit(1);
            }
        }
    }

    // Load files first (in order)
    for load_file in &cli.load {
        let path = std::path::Path::new(load_file);
        if let Ok(canonical) = path.canonicalize() {
            interpreter.ctx.push_file_path(canonical);
        }
        match read_source_file(load_file) {
            Ok(content) => {
                crate::set_last_input(&content, Some(PathBuf::from(load_file)));
                match interpreter.eval_str_compiled(&content) {
                    Ok(_) => {
                        interpreter.ctx.pop_file_path();
                        drain_async_scheduler(&interpreter);
                    }
                    Err(e) => {
                        interpreter.ctx.pop_file_path();
                        eprint!("Error loading {load_file}: ");
                        print_error(&e);
                        std::process::exit(1);
                    }
                }
            }
            Err(msg) => {
                die(msg);
            }
        }
    }

    // Handle --eval
    if let Some(expr) = &cli.eval {
        crate::set_last_input(expr, None);
        match interpreter.eval_str_compiled(expr) {
            Ok(val) => {
                drain_async_scheduler(&interpreter);
                if !val.is_nil() {
                    println!("{}", pretty_print(&val, 80));
                }
            }
            Err(e) => {
                print_error(&e);
                std::process::exit(1);
            }
        }
        if cli.interactive {
            repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
        }
        return;
    }

    // Handle --print
    if let Some(expr) = &cli.print {
        crate::set_last_input(expr, None);
        match interpreter.eval_str_compiled(expr) {
            Ok(val) => {
                drain_async_scheduler(&interpreter);
                println!("{val}");
            }
            Err(e) => {
                print_error(&e);
                std::process::exit(1);
            }
        }
        if cli.interactive {
            repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
        }
        return;
    }

    // Handle FILE
    if let Some(file) = &cli.file {
        let path = std::path::Path::new(file);

        // Auto-detect .semac bytecode files
        if let Ok(bytes) = std::fs::read(path) {
            if sema_vm::is_bytecode_file(&bytes) {
                match interpreter.run_bytecode_bytes(&bytes) {
                    Ok(_) => {
                        drain_async_scheduler(&interpreter);
                    }
                    Err(e) => {
                        print_error(&e);
                        std::process::exit(1);
                    }
                }
                if cli.interactive {
                    repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
                }
                return;
            }
        }

        if let Ok(canonical) = path.canonicalize() {
            interpreter.ctx.push_file_path(canonical);
        }
        match read_source_file(file) {
            Ok(content) => {
                crate::set_last_input(&content, Some(PathBuf::from(file)));
                match interpreter.eval_str_compiled(&content) {
                    Ok(_) => {
                        interpreter.ctx.pop_file_path();
                        drain_async_scheduler(&interpreter);
                    }
                    Err(e) => {
                        interpreter.ctx.pop_file_path();
                        print_error(&e);
                        std::process::exit(1);
                    }
                }
            }
            Err(msg) if msg.starts_with("file not found:") => {
                die(format!(
                    "file not found: '{file}' (not a file or command)\n\nRun 'sema --help' for available commands."
                ));
            }
            Err(msg) => {
                die(msg);
            }
        }
        if cli.interactive {
            repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
        }
        return;
    }

    // REPL mode
    repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
}

/// Drain any pending async tasks scheduled by a top-level form.
///
/// Retained as a call-site marker; under the unified cooperative runtime the
/// evaluator already drives every spawned task to completion during `eval`
/// (the persistent runtime drains to idle before returning), and any still-
/// detached task is reaped by the interpreter's bounded shutdown on drop. There
/// is no separate scheduler to run, so this is a no-op.
pub(crate) fn drain_async_scheduler(_interpreter: &Interpreter) {}

pub(crate) fn format_source_snippet(
    span: &sema_core::Span,
    file_override: Option<&std::path::Path>,
) -> Option<String> {
    let (source, file) = if let Some(path) = file_override {
        let content = std::fs::read_to_string(path).ok()?;
        (content, Some(path.to_path_buf()))
    } else {
        let (source, file) = last_input()?;
        (source, file)
    };

    let lines: Vec<&str> = source.lines().collect();
    let line_idx = span.line.checked_sub(1)?;
    let source_line = lines.get(line_idx)?;
    let col = span.col.saturating_sub(1);
    let line_num = span.line;
    let gutter_width = format!("{line_num}").len().max(2);
    let location = if let Some(path) = &file {
        format!("{}:{}:{}", path.display(), line_num, span.col)
    } else {
        format!("<input>:{}:{}", line_num, span.col)
    };

    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", colors::cyan("-->"), location));
    out.push_str(&format!("  {:>gutter_width$} {}\n", "", colors::cyan("|")));
    out.push_str(&format!(
        "  {} {} {}\n",
        colors::cyan(&format!("{:>gutter_width$}", line_num)),
        colors::cyan("|"),
        source_line
    ));
    out.push_str(&format!(
        "  {:>gutter_width$} {} {}{}",
        "",
        colors::cyan("|"),
        " ".repeat(col),
        colors::red_bold("^")
    ));
    Some(out)
}

pub(crate) fn print_cli_error(message: impl std::fmt::Display) {
    eprintln!("{} {message}", colors::red_bold("Error:"));
}

/// Print `message` as a CLI error and exit with status 1.
pub(crate) fn die(message: impl std::fmt::Display) -> ! {
    print_cli_error(message);
    std::process::exit(1)
}

pub(crate) fn print_cli_warning(message: impl std::fmt::Display) {
    eprintln!("{} {message}", colors::yellow("Warning:"));
}

pub(crate) fn print_error(e: &SemaError) {
    let inner = e.inner();
    print_cli_error(e.user_message());

    // Show source snippet for reader errors
    if let SemaError::Reader { span, .. } = inner {
        if let Some(snippet) = format_source_snippet(span, None) {
            eprintln!("{snippet}");
        }
    }

    if let Some(trace) = e.stack_trace() {
        // Show source context for innermost frame
        if let Some(first_frame) = trace.0.first() {
            if let Some(span) = &first_frame.span {
                let snippet = if first_frame.file.is_some() {
                    format_source_snippet(span, first_frame.file.as_deref())
                } else {
                    format_source_snippet(span, None)
                };
                if let Some(snippet) = snippet {
                    eprintln!("{snippet}");
                }
            }
        }

        for frame in &trace.0 {
            let loc = match (&frame.file, &frame.span) {
                (Some(file), Some(span)) => format!("({}:{span})", file.display()),
                (Some(file), None) => format!("({})", file.display()),
                (None, Some(span)) => format!("(<input>:{span})"),
                (None, None) => String::new(),
            };
            if loc.is_empty() {
                eprintln!("  {} {}", colors::dim("at"), frame.name);
            } else {
                eprintln!(
                    "  {} {} {}",
                    colors::dim("at"),
                    frame.name,
                    colors::dim(&loc)
                );
            }
        }
    }
    if let Some(hint) = e.hint() {
        eprintln!("  {} {hint}", colors::cyan("hint:"));
    }
    if let Some(note) = e.note() {
        eprintln!("  {} {note}", colors::yellow("note:"));
    }
}

fn run_doc(
    command: Option<DocCommands>,
    symbol: Option<String>,
    pager: docs::PagerMode,
) -> Result<(), String> {
    match command {
        Some(DocCommands::Show { symbol }) => show_doc(&symbol, pager),
        Some(DocCommands::Search { query, limit }) => {
            let query = query.join(" ");
            let query = query.trim().to_string();
            if query.is_empty() {
                return Err("usage: sema doc search <query>".to_string());
            }
            let rendered =
                docs::render_search_results(&query, &docs::doc_search_results(&query, limit));
            docs::print_rendered(&rendered, pager).map_err(|e| format!("writing docs: {e}"))
        }
        Some(DocCommands::Apropos { pattern }) => {
            let hits = docs::builtin_apropos_hits(&pattern);
            let rendered = docs::render_apropos_hits(&pattern, &hits);
            docs::print_rendered(&rendered, pager).map_err(|e| format!("writing docs: {e}"))
        }
        None => {
            let Some(symbol) = symbol else {
                return Err("usage: sema doc <symbol> | sema doc search <query> | sema doc apropos <pattern>".to_string());
            };
            show_doc(&symbol, pager)
        }
    }
}

fn show_doc(symbol: &str, pager: docs::PagerMode) -> Result<(), String> {
    let Some(rendered) = docs::rendered_doc(symbol) else {
        return Err(format!("documentation not found: {symbol}"));
    };
    docs::print_rendered(&rendered, pager).map_err(|e| format!("writing docs: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_input_setter_getter_round_trip() {
        // The pair must be written and read atomically: whatever file was set
        // last is what the getter returns, never a mix of two producers.
        set_last_input("with file", Some(PathBuf::from("a.sema")));
        assert_eq!(
            last_input(),
            Some(("with file".to_string(), Some(PathBuf::from("a.sema"))))
        );

        set_last_input("no file", None);
        assert_eq!(last_input(), Some(("no file".to_string(), None)));

        // Overwriting with a new pair replaces both halves.
        set_last_input("with file again", Some(PathBuf::from("b.sema")));
        assert_eq!(
            last_input(),
            Some(("with file again".to_string(), Some(PathBuf::from("b.sema"))))
        );
    }
}
