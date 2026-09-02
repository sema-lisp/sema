use std::cell::RefCell;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use sema_core::{archive, pretty_print, SemaError, Value, ValueView};
use sema_eval::Interpreter;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct SemaConfig {
    #[serde(default)]
    fmt: FmtConfig,
}

#[derive(Debug, Deserialize)]
struct FmtConfig {
    #[serde(default = "default_width")]
    width: usize,
    #[serde(default = "default_indent")]
    indent: usize,
    #[serde(default)]
    align: bool,
    #[serde(
        default = "default_max_blank_lines",
        alias = "max_blank_lines",
        rename = "max-blank-lines"
    )]
    max_blank_lines: usize,
    /// Glob patterns (or literal path prefixes) excluded from formatting.
    /// Explicitly named files bypass this list.
    #[serde(default)]
    ignore: Vec<String>,
}

impl Default for FmtConfig {
    fn default() -> Self {
        Self {
            width: 80,
            indent: 2,
            align: false,
            max_blank_lines: 1,
            ignore: Vec::new(),
        }
    }
}

fn default_width() -> usize {
    sema_fmt::FormatOptions::default().width
}
fn default_indent() -> usize {
    sema_fmt::FormatOptions::default().indent
}
fn default_max_blank_lines() -> usize {
    sema_fmt::FormatOptions::default().max_blank_lines
}

/// Walk up from cwd to find sema.toml
fn find_config() -> Option<SemaConfig> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("sema.toml");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate).ok()?;
            return toml::from_str(&text).ok();
        }
        if !dir.pop() {
            return None;
        }
    }
}

mod colors;
mod cross_compile;
mod docs;
mod import_tracer;
mod pkg;
mod repl;
mod update;
mod web;
mod workflow_check;
// The dashboard server itself lives in the `sema` LIBRARY crate
// (`crates/sema/src/lib.rs` → `pub mod workflow_view;`), not here, so
// `crates/sema/tests/*.rs` integration tests can drive it in-process. Referenced
// below as `sema::workflow_view::…`.
use sema::workflow_view;

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

/// Build the standard CLI interpreter: stdlib + LLM (registered inside sema-eval)
/// plus the MCP *client* builtins (`mcp/connect`, `mcp/tools`, `mcp/tools->sema`,
/// …). The MCP builtins live in `sema-mcp`, which depends on `sema-eval`, so they
/// can't be registered inside `sema-eval` itself — the binary wires them in here.
/// The real `WorkflowMcpResolver` (`sema::workflow_mcp`) is registered right
/// alongside them, so every CLI path built through this function (REPL, `sema
/// run`, `sema workflow run`, …) can resolve a workflow's declared `:mcp`
/// servers — see docs/plans/2026-06-24-workflow-mcp-auth.md §3/§9(a).
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
    use super::{
        format_needs_approval_guidance, is_double_interrupt, resolve_approval_mode, shell_quote,
        terminal_safe, validate_interactive_approval_authority, ApprovalMode, Value,
    };
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
    if let Some(exit_code) = try_run_embedded() {
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

    let sandbox = match &cli.sandbox {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            print_cli_error(e);
            std::process::exit(1);
        }),
        None => sema_core::Sandbox::allow_all(),
    };
    let sandbox = match &cli.allowed_paths {
        Some(value) => {
            let paths = sema_core::Sandbox::parse_allowed_paths(value);
            sandbox.with_allowed_paths(paths)
        }
        None => sandbox,
    };

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::Ast { file, eval, json } => {
                run_ast(file, eval, json);
            }
            Commands::Completions { shell, install } => {
                if install {
                    install_completions(shell);
                } else {
                    print!("{}", generate_completions(shell));
                }
            }
            Commands::Compile {
                file,
                output,
                check,
            } => {
                if check {
                    run_check(&file);
                } else {
                    run_compile(&file, output.as_deref());
                }
            }
            Commands::Disasm { file, json } => {
                run_disasm(&file, json);
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
                    print_cli_error(msg);
                    std::process::exit(1);
                }
            }
            Commands::Pkg { json, command } => {
                let result = match command {
                    PkgCommands::Add { spec, registry } => {
                        pkg::cmd_add(&spec, registry.as_deref(), json)
                    }
                    PkgCommands::Install { locked } => pkg::cmd_install(locked, json),
                    PkgCommands::Update { name } => pkg::cmd_update(name.as_deref(), json),
                    PkgCommands::Remove { name } => pkg::cmd_remove(&name, json),
                    PkgCommands::List => pkg::cmd_list(json),
                    PkgCommands::Init => pkg::cmd_init(json),
                    PkgCommands::Login {
                        token,
                        username,
                        password,
                        registry,
                    } => pkg::cmd_login(
                        token.as_deref(),
                        username.as_deref(),
                        password.as_deref(),
                        &registry,
                        json,
                    ),
                    PkgCommands::Logout => pkg::cmd_logout(json),
                    PkgCommands::Whoami { registry } => pkg::cmd_whoami(registry.as_deref(), json),
                    PkgCommands::Config { key, value } => {
                        pkg::cmd_config(key.as_deref(), value.as_deref(), json)
                    }
                    PkgCommands::Publish { registry } => {
                        pkg::cmd_publish(registry.as_deref(), json)
                    }
                    PkgCommands::Search { query, registry } => {
                        pkg::cmd_search(&query, registry.as_deref(), json)
                    }
                    PkgCommands::Yank { spec, registry } => {
                        pkg::cmd_yank(&spec, registry.as_deref(), json)
                    }
                    PkgCommands::Info { name, registry } => {
                        pkg::cmd_info(&name, registry.as_deref(), json)
                    }
                };
                if let Err(e) = result {
                    print_cli_error(e);
                    std::process::exit(1);
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
                if let Err(e) = run_build(
                    &file,
                    output.as_deref(),
                    &includes,
                    runtime.as_deref(),
                    target.as_deref(),
                    no_cache,
                    BuildOutputOpts { verbose, json },
                ) {
                    print_cli_error(e);
                    std::process::exit(1);
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
                let config = find_config().unwrap_or_default();
                let opts = sema_fmt::FormatOptions {
                    width: width.unwrap_or(config.fmt.width),
                    indent: indent.unwrap_or(config.fmt.indent),
                    align: align || config.fmt.align,
                    max_blank_lines: max_blank_lines.unwrap_or(config.fmt.max_blank_lines),
                };
                run_fmt(&files, check, diff, &opts, &config.fmt.ignore, json);
            }
            Commands::Lsp => {
                eprintln!("Sema LSP server starting on stdio...");
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime")
                    .block_on(sema_lsp::run_server());
            }
            Commands::Dap => {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime")
                    .block_on(sema_dap::run_server());
            }
            Commands::Mcp {
                auth,
                files,
                include,
                exclude,
                sandbox: mcp_sandbox,
                timeout_ms,
            } => {
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
                        print_cli_error(format!("MCP command failed: {e}"));
                        std::process::exit(1);
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
                        print_cli_error(format!("invalid MCP --sandbox value: {e}"));
                        std::process::exit(1);
                    }),
                    None => sandbox,
                };
                let interpreter = build_interpreter(&sandbox);

                let _ = interpreter.eval_str("(llm/auto-configure)");

                for file in files {
                    match read_source_file(&file) {
                        Ok(content) => {
                            if let Err(e) = interpreter.eval_str_compiled(&content) {
                                print_cli_error(format!("could not load tool file {file}: {e}"));
                                std::process::exit(1);
                            }
                        }
                        Err(e) => {
                            print_cli_error(format!("could not read tool file {file}: {e}"));
                            std::process::exit(1);
                        }
                    }
                }

                // Sync loop, deliberately NO tokio runtime: with an ambient
                // runtime, llm/* builtins hit io_block_on's runtime-in-runtime
                // panic and killed the server on the first LLM tool call.
                let tool_timeout = resolve_mcp_tool_timeout(timeout_ms);
                if let Err(e) =
                    sema_mcp::run_mcp_server_sync(interpreter, inc_tools, exc_tools, tool_timeout)
                {
                    print_cli_error(format!("MCP server failed: {e}"));
                    std::process::exit(1);
                }
            }
            Commands::Notebook { command } => {
                run_notebook_command(command);
            }
            Commands::Web {
                file,
                host,
                port,
                no_open,
                no_llm,
            } => {
                if let Err(e) = web::run(&file, &host, port, !no_open, !no_llm) {
                    print_cli_error(format!("sema web failed: {e}"));
                    std::process::exit(1);
                }
            }
            Commands::Workflow { command } => {
                run_workflow_command(command, &sandbox);
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
                run_eval(stdin, expr, json, path, timeout, sandbox, no_llm);
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
                    print_cli_error(e);
                    std::process::exit(1);
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
                print_cli_error(msg);
                std::process::exit(1);
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
                match run_bytecode_bytes(&interpreter, &bytes) {
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
                print_cli_error(format!(
                    "file not found: '{file}' (not a file or command)\n\nRun 'sema --help' for available commands."
                ));
                std::process::exit(1);
            }
            Err(msg) => {
                print_cli_error(msg);
                std::process::exit(1);
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

fn default_approval_actor() -> String {
    std::env::var("SEMA_APPROVAL_ACTOR")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "local-user".to_string())
}

fn read_approval_key_file(path: &Path, private: bool) -> Result<String, String> {
    use std::io::Read as _;

    const MAX_KEY_BYTES: u64 = 16 * 1024;
    let file = if private {
        sema_workflow::approval::open_private_file(path)
    } else {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        options.open(path)
    }
    .map_err(|error| format!("cannot open approval key {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect approval key {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_KEY_BYTES {
        return Err(format!(
            "{} is too large to be an approval key",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_KEY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read approval key {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_KEY_BYTES {
        return Err(format!(
            "{} is too large to be an approval key",
            path.display()
        ));
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| format!("approval key {} is not UTF-8", path.display()))?;
    if private && value.trim().is_empty() {
        return Err("approval signing key file is empty".to_string());
    }
    Ok(value.trim().to_string())
}

fn load_approval_signing_key(
    path: &Path,
) -> Result<sema_workflow::approval::ApprovalSigningKey, String> {
    let encoded = read_approval_key_file(path, true)?;
    sema_workflow::approval::ApprovalSigningKey::from_base64(&encoded)
        .map_err(|error| error.to_string())
}

fn create_approval_key_pair(private_path: &Path, public_path: &Path) -> Result<(), String> {
    use std::io::Write as _;

    if private_path == public_path {
        return Err("private and public approval key paths must differ".to_string());
    }
    let key = sema_workflow::approval::ApprovalSigningKey::generate()
        .map_err(|error| error.to_string())?;
    let mut private = sema_workflow::approval::create_private_file_new(private_path)
        .map_err(|error| format!("cannot create {}: {error}", private_path.display()))?;
    let mut public_file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(public_path)
    {
        Ok(file) => file,
        Err(error) => {
            drop(private);
            let _ = std::fs::remove_file(private_path);
            return Err(format!("cannot create {}: {error}", public_path.display()));
        }
    };
    let write_result = private
        .write_all(format!("{}\n", key.to_base64()).as_bytes())
        .and_then(|()| private.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", private_path.display()))
        .and_then(|()| {
            let public = key.public_key_base64().map_err(|error| error.to_string())?;
            public_file
                .write_all(format!("{public}\n").as_bytes())
                .and_then(|()| public_file.sync_all())
                .map_err(|error| format!("cannot write {}: {error}", public_path.display()))
        });
    drop(private);
    drop(public_file);
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(public_path);
        let _ = std::fs::remove_file(private_path);
        return Err(error);
    }
    Ok(())
}

struct WorkflowApprovalRevision {
    digest: String,
    embedded_dependencies: Vec<(PathBuf, Vec<u8>)>,
}

fn workflow_approval_revision(
    file: &Path,
    content: &[u8],
) -> Result<WorkflowApprovalRevision, String> {
    use sha2::{Digest as _, Sha256};

    const MAX_FILES: usize = 4096;
    const MAX_BYTES: usize = 64 * 1024 * 1024;

    let snapshot = import_tracer::trace_imports_strict(file)?;
    let mut dependencies = snapshot.files.into_iter().collect::<Vec<_>>();
    dependencies.sort_by(|left, right| left.0.cmp(&right.0));
    if dependencies.len() > MAX_FILES {
        return Err(format!(
            "workflow dependency closure exceeds {MAX_FILES} files"
        ));
    }

    let mut manifests = Vec::with_capacity(2);

    let mut ancestor = file
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize {}: {error}", file.display()))?
        .parent()
        .map(Path::to_path_buf);
    while let Some(directory) = ancestor {
        for name in ["sema.toml", "sema.lock"] {
            let path = directory.join(name);
            if path.is_file()
                && !dependencies.iter().any(|(key, _)| key == name)
                && !manifests.iter().any(|(key, _)| key == name)
            {
                let bytes = std::fs::read(&path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                manifests.push((name.to_string(), bytes));
            }
        }
        ancestor = directory.parent().map(Path::to_path_buf);
    }

    let mut inputs = Vec::with_capacity(dependencies.len() + manifests.len() + 1);
    inputs.push(("<entry>", content));
    inputs.extend(
        dependencies
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    );
    inputs.extend(
        manifests
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    );
    inputs.sort_by(|left, right| left.0.cmp(right.0));

    let total_bytes = inputs.iter().try_fold(0usize, |total, (_, bytes)| {
        total
            .checked_add(bytes.len())
            .ok_or_else(|| "workflow dependency closure byte count overflowed".to_string())
    })?;
    if total_bytes > MAX_BYTES {
        return Err(format!(
            "workflow dependency closure exceeds {MAX_BYTES} bytes"
        ));
    }

    let mut digest = Sha256::new();
    digest.update(b"sema-workflow-approval-revision-v1");
    for (name, bytes) in inputs {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }

    let mut embedded_dependencies = std::collections::BTreeMap::new();
    for (name, bytes) in dependencies {
        embedded_dependencies.insert(PathBuf::from(name), bytes);
    }
    for (filesystem_path, bytes) in snapshot.filesystem_files {
        match embedded_dependencies.entry(filesystem_path.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(bytes.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &bytes => {}
            std::collections::btree_map::Entry::Occupied(entry) => {
                return Err(format!(
                    "dependency snapshot key collision at {}",
                    entry.key().display()
                ));
            }
        }
        let alias = sema_core::vfs::normalize_path(&filesystem_path).ok_or_else(|| {
            format!(
                "cannot normalize imported file identity {}",
                filesystem_path.display()
            )
        })?;
        match embedded_dependencies.entry(PathBuf::from(alias)) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(bytes);
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &bytes => {}
            std::collections::btree_map::Entry::Occupied(entry) => {
                return Err(format!(
                    "dependency snapshot key collision at {}",
                    entry.key().display()
                ));
            }
        }
    }

    Ok(WorkflowApprovalRevision {
        digest: format!("{:x}", digest.finalize()),
        embedded_dependencies: embedded_dependencies.into_iter().collect(),
    })
}

struct WorkflowApprovalInput<'a> {
    runs_root: &'a Path,
    run_id: &'a str,
    approval_id: &'a str,
    signing_key: &'a sema_workflow::approval::ApprovalSigningKey,
    kind: sema_workflow::approval::ApprovalDecisionKind,
    actor: String,
    comment: Option<String>,
    reason: Option<String>,
}

fn write_workflow_approval(input: WorkflowApprovalInput<'_>) {
    match sema_workflow::approval::decide(
        input.runs_root,
        input.run_id,
        input.approval_id,
        input.signing_key,
        input.kind,
        input.actor,
        "cli".to_string(),
        input.comment,
        input.reason,
    ) {
        Ok(sema_workflow::approval::DecisionWrite::Created(decision)) => println!(
            "{} {} ({})",
            approval_decision_past_tense(decision.decision),
            decision.approval_id,
            decision.decision_id
        ),
        Ok(sema_workflow::approval::DecisionWrite::AlreadyExists(decision)) => println!(
            "already {} {} ({})",
            approval_decision_past_tense(decision.decision),
            decision.approval_id,
            decision.decision_id
        ),
        Err(error) => {
            print_cli_error(format!(
                "cannot {} approval {}: {error}",
                input.kind, input.approval_id
            ));
            std::process::exit(1);
        }
    }
}

fn approval_decision_past_tense(
    decision: sema_workflow::approval::ApprovalDecisionKind,
) -> &'static str {
    match decision {
        sema_workflow::approval::ApprovalDecisionKind::Approve => "approved",
        sema_workflow::approval::ApprovalDecisionKind::Reject => "rejected",
    }
}

fn approval_resolution_json(
    resolution: &sema_workflow::approval::ApprovalResolution,
) -> serde_json::Value {
    use sema_workflow::approval::ApprovalResolution;
    let (status, request, decision) = match resolution {
        ApprovalResolution::Pending(request) => ("pending", request, None),
        ApprovalResolution::Approved(request, decision) => ("approved", request, Some(decision)),
        ApprovalResolution::Rejected(request, decision) => ("rejected", request, Some(decision)),
    };
    serde_json::json!({
        "status": status,
        "request": request,
        "decision": decision,
    })
}

fn list_workflow_approvals(runs_root: &Path, run_id: &str, json: bool) {
    let resolutions = match sema_workflow::approval::list_requests(runs_root, run_id) {
        Ok(resolutions) => resolutions,
        Err(error) => {
            print_cli_error(format!("cannot list approvals for {run_id}: {error}"));
            std::process::exit(1);
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &resolutions
                    .iter()
                    .map(approval_resolution_json)
                    .collect::<Vec<_>>()
            )
            .expect("approval list is JSON serializable")
        );
        return;
    }
    if resolutions.is_empty() {
        println!("no approvals for run {run_id}");
        return;
    }
    for resolution in &resolutions {
        let json = approval_resolution_json(resolution);
        let request = &json["request"];
        println!(
            "{}  {:<8}  {}",
            request["approval_id"].as_str().unwrap_or(""),
            json["status"].as_str().unwrap_or("unknown"),
            terminal_safe(request["reason"].as_str().unwrap_or(""))
        );
        if let Some(preview) = request["preview"].as_str() {
            println!("  {}", terminal_safe(preview));
        }
    }
}

enum ApprovalPromptAction {
    Approve,
    Reject(String),
    AlreadyDecided,
    LeavePending,
}

fn is_terminal_control(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{206f}'
        )
}

fn terminal_safe(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for ch in value.chars() {
        if is_terminal_control(ch) {
            use std::fmt::Write as _;
            let _ = write!(safe, "\\u{{{:x}}}", ch as u32);
        } else {
            safe.push(ch);
        }
    }
    safe
}

fn prompt_for_workflow_approval(
    runs_root: &Path,
    run_id: &str,
    approval_id: &str,
) -> Result<ApprovalPromptAction, String> {
    use std::io::Write;

    let resolution = sema_workflow::approval::list_requests(runs_root, run_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|resolution| {
            let request = match resolution {
                sema_workflow::approval::ApprovalResolution::Pending(request)
                | sema_workflow::approval::ApprovalResolution::Approved(request, _)
                | sema_workflow::approval::ApprovalResolution::Rejected(request, _) => request,
            };
            request.approval_id == approval_id
        })
        .ok_or_else(|| format!("approval {approval_id} was not found"))?;
    let request = match resolution {
        sema_workflow::approval::ApprovalResolution::Pending(request) => request,
        sema_workflow::approval::ApprovalResolution::Approved(_, decision)
        | sema_workflow::approval::ApprovalResolution::Rejected(_, decision) => {
            eprintln!(
                "approval {approval_id} was already {} by {}",
                decision.decision,
                terminal_safe(&decision.actor)
            );
            return Ok(ApprovalPromptAction::AlreadyDecided);
        }
    };

    let _ctrlc_guard = ApprovalPromptCtrlCGuard::enter();
    eprintln!("\nHuman approval required");
    eprintln!("  id:      {}", request.approval_id);
    eprintln!("  reason:  {}", terminal_safe(&request.reason));
    if let Some(preview) = request.preview {
        eprintln!("  preview: {}", terminal_safe(&preview));
    }
    loop {
        eprint!("Approve? [y]es / [n]o / [q]uit pending: ");
        std::io::stderr()
            .flush()
            .map_err(|error| error.to_string())?;
        let mut answer = String::new();
        let read = std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(ApprovalPromptAction::LeavePending);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(ApprovalPromptAction::Approve),
            "n" | "no" => {
                eprint!("Rejection reason: ");
                std::io::stderr()
                    .flush()
                    .map_err(|error| error.to_string())?;
                let mut reason = String::new();
                if std::io::stdin()
                    .read_line(&mut reason)
                    .map_err(|error| error.to_string())?
                    == 0
                {
                    return Ok(ApprovalPromptAction::LeavePending);
                }
                if reason.trim().is_empty() {
                    eprintln!("A rejection reason is required.");
                    continue;
                }
                return Ok(ApprovalPromptAction::Reject(reason.trim().to_string()));
            }
            "q" | "quit" => return Ok(ApprovalPromptAction::LeavePending),
            _ => eprintln!("Enter y, n, or q."),
        }
    }
}

fn approval_was_decided(runs_root: &Path, run_id: &str, approval_id: &str) -> bool {
    sema_workflow::approval::list_requests(runs_root, run_id)
        .ok()
        .is_some_and(|resolutions| {
            resolutions.into_iter().any(|resolution| match resolution {
                sema_workflow::approval::ApprovalResolution::Pending(_) => false,
                sema_workflow::approval::ApprovalResolution::Approved(request, _)
                | sema_workflow::approval::ApprovalResolution::Rejected(request, _) => {
                    request.approval_id == approval_id
                }
            })
        })
}

fn approval_envelope_field(envelope: &Value, key: &str) -> Option<String> {
    envelope
        .as_map_rc()?
        .get(&Value::keyword(key))?
        .as_str()
        .map(str::to_string)
}

fn shell_quote(value: &str) -> String {
    #[cfg(windows)]
    {
        return format!("\"{}\"", value.replace('"', "\\\""));
    }
    #[cfg(not(windows))]
    {
        use std::fmt::Write as _;

        // ANSI-C quotes preserve exact control/bidi characters without rendering them
        // into the terminal. Sema's supported Unix shells (bash/zsh) understand this
        // form, including \xHH/\uHHHH/\UHHHHHHHH escapes.
        let mut quoted = String::with_capacity(value.len() + 3);
        quoted.push_str("$'");
        for ch in value.chars() {
            match ch {
                '\\' => quoted.push_str("\\\\"),
                '\'' => quoted.push_str("\\'"),
                _ if is_terminal_control(ch) => {
                    let scalar = ch as u32;
                    if scalar <= 0xff {
                        let _ = write!(quoted, "\\x{scalar:02x}");
                    } else if scalar <= 0xffff {
                        let _ = write!(quoted, "\\u{scalar:04x}");
                    } else {
                        let _ = write!(quoted, "\\U{scalar:08x}");
                    }
                }
                _ => quoted.push(ch),
            }
        }
        quoted.push('\'');
        quoted
    }
}

fn format_needs_approval_guidance(
    envelope: &Value,
    run_dir: &str,
    file: &str,
    args: &str,
    public_key_file: Option<&str>,
    signing_key_file: Option<&str>,
) -> String {
    let run_id = approval_envelope_field(envelope, "run-id").unwrap_or_default();
    let approval_id = approval_envelope_field(envelope, "approval-id").unwrap_or_default();
    let mut out = format!(
        "run needs human approval {approval_id}:\n  sema workflow approve {} {} --run-dir {} --signing-key-file \"$SEMA_APPROVAL_PRIVATE_KEY\"\n  sema workflow reject {} {} --run-dir {} --reason 'explain why' --signing-key-file \"$SEMA_APPROVAL_PRIVATE_KEY\"\n",
        shell_quote(&run_id),
        shell_quote(&approval_id),
        shell_quote(run_dir),
        shell_quote(&run_id),
        shell_quote(&approval_id),
        shell_quote(run_dir),
    );
    if let Some(signing_key_file) = signing_key_file {
        out.push_str(&format!(
            "or decide in the loopback viewer, then resume exactly:\n  sema workflow run {} --args {} --resume {} --run-dir {} --view --approval-signing-key-file {}\n",
            shell_quote(file),
            shell_quote(args),
            shell_quote(&run_id),
            shell_quote(run_dir),
            shell_quote(signing_key_file),
        ));
    } else if let Some(public_key_file) = public_key_file {
        out.push_str(&format!(
            "then resume exactly:\n  sema workflow run {} --args {} --resume {} --run-dir {} --approval-public-key-file {}\n",
            shell_quote(file),
            shell_quote(args),
            shell_quote(&run_id),
            shell_quote(run_dir),
            shell_quote(public_key_file),
        ));
    } else {
        out.push_str(
            "this interactive request used an ephemeral authority; if left pending, start a fresh run instead of resuming it.\n",
        );
    }
    out
}

/// `sema workflow run <file>` — evaluate a workflow `.sema` file (which
/// `defworkflow`s and runs it) with the run-directory + args seams wired, then
/// exit non-zero if the run's `{:status …}` envelope reports failure.
fn run_workflow_command(command: WorkflowCommands, sandbox: &sema_core::Sandbox) {
    let (
        file,
        args,
        run_dir,
        view,
        view_port,
        resume,
        no_auth_prompt,
        approval_mode,
        approval_public_key_file,
        approval_signing_key_file,
        approval_actor,
    ) = match command {
        WorkflowCommands::Run {
            file,
            args,
            run_dir,
            view,
            port,
            resume,
            no_auth_prompt,
            approval_mode,
            approval_public_key_file,
            approval_signing_key_file,
            approval_actor,
        } => (
            file,
            args,
            run_dir,
            view,
            port,
            resume,
            no_auth_prompt,
            approval_mode,
            approval_public_key_file,
            approval_signing_key_file,
            approval_actor,
        ),
        WorkflowCommands::Approvals {
            run_id,
            run_dir,
            json,
        } => {
            list_workflow_approvals(Path::new(&run_dir), &run_id, json);
            return;
        }
        WorkflowCommands::Approve {
            run_id,
            approval_id,
            run_dir,
            comment,
            actor,
            signing_key_file,
        } => {
            let signing_key = load_approval_signing_key(Path::new(&signing_key_file))
                .unwrap_or_else(|error| {
                    print_cli_error(error);
                    std::process::exit(1);
                });
            write_workflow_approval(WorkflowApprovalInput {
                runs_root: Path::new(&run_dir),
                run_id: &run_id,
                approval_id: &approval_id,
                signing_key: &signing_key,
                kind: sema_workflow::approval::ApprovalDecisionKind::Approve,
                actor: actor.unwrap_or_else(default_approval_actor),
                comment,
                reason: None,
            });
            return;
        }
        WorkflowCommands::Reject {
            run_id,
            approval_id,
            run_dir,
            reason,
            actor,
            signing_key_file,
        } => {
            let signing_key = load_approval_signing_key(Path::new(&signing_key_file))
                .unwrap_or_else(|error| {
                    print_cli_error(error);
                    std::process::exit(1);
                });
            write_workflow_approval(WorkflowApprovalInput {
                runs_root: Path::new(&run_dir),
                run_id: &run_id,
                approval_id: &approval_id,
                signing_key: &signing_key,
                kind: sema_workflow::approval::ApprovalDecisionKind::Reject,
                actor: actor.unwrap_or_else(default_approval_actor),
                comment: None,
                reason: Some(reason),
            });
            return;
        }
        WorkflowCommands::ApprovalKeygen {
            private_key_file,
            public_key_file,
        } => {
            if let Err(error) =
                create_approval_key_pair(Path::new(&private_key_file), Path::new(&public_key_file))
            {
                print_cli_error(error);
                std::process::exit(1);
            }
            println!("created private approval key {private_key_file}");
            println!("created public approval key  {public_key_file}");
            return;
        }
        WorkflowCommands::View {
            run_dir,
            host,
            port,
            approval_signing_key_file,
            approval_actor,
        } => {
            let approval_authority = approval_signing_key_file
                .as_deref()
                .map(|path| {
                    let signing_key = load_approval_signing_key(Path::new(path))?;
                    workflow_view::ApprovalAuthority::new(
                        signing_key,
                        approval_actor.unwrap_or_else(default_approval_actor),
                    )
                    .map_err(|error| error.to_string())
                })
                .transpose()
                .unwrap_or_else(|error| {
                    print_cli_error(format!("cannot enable viewer approval controls: {error}"));
                    std::process::exit(1);
                });
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(workflow_view::serve_with_approval(
                    PathBuf::from(run_dir),
                    &host,
                    port,
                    approval_authority,
                ));
            return;
        }
        WorkflowCommands::Index { run_dir } => {
            let root = PathBuf::from(&run_dir);
            match workflow_view::ingest::open(&root.join(sema_workflow::INDEX_DB)) {
                Ok(conn) => {
                    workflow_view::ingest::backfill_all(&conn, &root);
                    match workflow_view::ingest::runs_summary(&conn) {
                        Ok(rows) => println!(
                            "indexed {} run(s) → {}",
                            rows.len(),
                            root.join(sema_workflow::INDEX_DB).display()
                        ),
                        Err(e) => print_cli_warning(format!("could not summarize index: {e}")),
                    }
                }
                Err(e) => {
                    print_cli_error(format!("cannot open index database: {e}"));
                    std::process::exit(1);
                }
            }
            return;
        }
        WorkflowCommands::Export {
            run_id,
            run_dir,
            out_dir,
        } => {
            match sema::workflow_evidence::export(
                &PathBuf::from(run_dir),
                &run_id,
                out_dir.as_deref().map(std::path::Path::new),
            ) {
                Ok(bundle) => {
                    println!(
                        "exported workflow evidence → {}",
                        bundle.directory.display()
                    );
                    println!("  {}", bundle.evidence_json.display());
                    println!("  {}", bundle.evidence_markdown.display());
                    println!("  {}", bundle.manifest_json.display());
                }
                Err(error) => {
                    print_cli_error(format!("cannot export workflow evidence: {error}"));
                    std::process::exit(1);
                }
            }
            return;
        }
        WorkflowCommands::Check { file, strict, json } => {
            let src = match read_source_file(&file) {
                Ok(s) => s,
                Err(msg) => {
                    print_cli_error(msg);
                    std::process::exit(2);
                }
            };
            let diags = workflow_check::check_run_source(&src);
            std::process::exit(workflow_check::report(&file, &diags, strict, json));
        }
    };

    let workspace_root = std::env::current_dir().unwrap_or_else(|error| {
        print_cli_error(format!("cannot resolve workflow workspace: {error}"));
        std::process::exit(1);
    });
    let run_dir = {
        let path = PathBuf::from(&run_dir);
        let absolute = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        absolute.to_string_lossy().to_string()
    };
    let file = std::fs::canonicalize(&file).unwrap_or_else(|error| {
        print_cli_error(format!("cannot resolve workflow file {file}: {error}"));
        std::process::exit(1);
    });
    let file = file.to_string_lossy().to_string();
    let approval_public_key_file = approval_public_key_file.map(|value| {
        let path = PathBuf::from(value);
        let absolute = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        absolute.to_string_lossy().to_string()
    });
    let approval_signing_key_file = approval_signing_key_file.map(|value| {
        let path = PathBuf::from(value);
        let absolute = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        absolute.to_string_lossy().to_string()
    });

    // Interactive MCP auth (docs/plans/2026-06-24-workflow-mcp-auth.md §3): on a
    // real terminal, a needs-auth gate logs in inline instead of exiting 2. See
    // `should_enable_interactive_auth` for the exact decision and
    // `sema::workflow_mcp::set_interactive_auth` for what enabling it does.
    sema::workflow_mcp::set_interactive_auth(should_enable_interactive_auth(
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
        std::env::var("CI").ok().as_deref(),
        no_auth_prompt,
    ));

    // Backward-compatible host seam for deterministic tests/embedders. Read exactly
    // once before evaluation and copy into host-owned state; a workflow's later
    // `sys/set-env` calls cannot alter it.
    let fresh_run_id = std::env::var("SEMA_WORKFLOW_RUN_ID")
        .ok()
        .filter(|value| !value.is_empty());
    if let Some(run_id) = &fresh_run_id {
        if run_id.contains('/') || run_id.contains('\\') || run_id.contains("..") {
            print_cli_error(
                "SEMA_WORKFLOW_RUN_ID must be a bare directory name without path separators",
            );
            std::process::exit(1);
        }
    }

    // `--resume <run-id>`: reuse that run's dir + memo cache. Sanitize the operator-
    // supplied id against path traversal (it joins into a filesystem path) and require
    // the prior run's events.jsonl to exist. The values are installed below in immutable
    // host state rather than mutable process environment variables.
    if let Some(run_id) = &resume {
        if run_id.is_empty()
            || run_id.contains('/')
            || run_id.contains('\\')
            || run_id.contains("..")
        {
            print_cli_error(
                "--resume run-id must be a bare directory name without path separators",
            );
            std::process::exit(1);
        }
        let prior = PathBuf::from(&run_dir).join(run_id).join("events.jsonl");
        if !prior.exists() {
            print_cli_error(format!("no prior run to resume at {}", prior.display()));
            std::process::exit(1);
        }
    }

    let content = match read_source_file(&file) {
        Ok(c) => c,
        Err(msg) => {
            print_cli_error(msg);
            std::process::exit(1);
        }
    };
    let run_diags = workflow_check::check_run_source(&content);
    if run_diags
        .iter()
        .any(|diag| diag.severity == workflow_check::Severity::Error)
    {
        let code = workflow_check::report(&file, &run_diags, false, false);
        std::process::exit(code.max(1));
    }

    let approval_revision = workflow_approval_revision(Path::new(&file), content.as_bytes())
        .unwrap_or_else(|error| {
            print_cli_error(format!(
                "cannot bind workflow approval dependency closure: {error}"
            ));
            std::process::exit(1);
        });
    let approval_code_version = approval_revision.digest.clone();

    let mut effective_sandbox = sandbox.clone();
    let permission_specs = match workflow_check::declared_permission_specs(&content) {
        Ok(specs) => specs,
        Err(e) => {
            print_cli_error(format!("invalid workflow permissions: {e}"));
            std::process::exit(1);
        }
    };
    for spec in permission_specs {
        let declared = sema_core::Sandbox::parse_cli(&spec).unwrap_or_else(|e| {
            print_cli_error(format!("invalid defworkflow :permissions {spec:?}: {e}"));
            std::process::exit(1);
        });
        effective_sandbox = effective_sandbox.with_more_denied(declared.denied);
    }

    // Bind the parsed --args JSON object to the global `*workflow-args*` so the
    // workflow body can read its inputs.
    let args_value = match serde_json::from_str::<serde_json::Value>(&args) {
        Ok(json) => sema_core::json::json_to_value(&json),
        Err(e) => {
            print_cli_error(format!("--args is not valid JSON: {e}"));
            std::process::exit(1);
        }
    };
    // Preserve the established short source hash for memo compatibility. Approval
    // decisions use a separate collision-resistant source binding below.
    let code_version = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };

    // The file's last form is the `defworkflow` (which expands to `workflow/run`),
    // so eval returns the `{:status …}` envelope; journaling is its side effect. A TTY
    // approval decision starts a fresh interpreter and resumes the same run. Completed
    // leaves replay from their memos; no interpreter-local continuation is trusted.
    let stdin_tty = std::io::stdin().is_terminal();
    let stderr_tty = std::io::stderr().is_terminal();
    let interactive_approval = stdin_tty
        && stderr_tty
        && std::env::var("CI")
            .ok()
            .as_deref()
            .is_none_or(str::is_empty);
    let viewer_signing_key = approval_signing_key_file
        .as_deref()
        .map(|path| load_approval_signing_key(Path::new(path)))
        .transpose()
        .unwrap_or_else(|error| {
            print_cli_error(format!("cannot enable viewer approval controls: {error}"));
            std::process::exit(1);
        });
    let viewer_public_key = viewer_signing_key.as_ref().map(|key| {
        key.public_key_base64().unwrap_or_else(|error| {
            print_cli_error(format!("cannot derive viewer approval key: {error}"));
            std::process::exit(1);
        })
    });
    let configured_public_key = approval_public_key_file.as_deref().map(|path| {
        let encoded = read_approval_key_file(Path::new(path), false).unwrap_or_else(|error| {
            print_cli_error(error);
            std::process::exit(1);
        });
        sema_workflow::approval::normalize_public_key_base64(&encoded).unwrap_or_else(|error| {
            print_cli_error(format!("invalid approval public key: {error}"));
            std::process::exit(1);
        })
    });
    if viewer_public_key
        .as_ref()
        .zip(configured_public_key.as_ref())
        .is_some_and(|(viewer, configured)| viewer != configured)
    {
        print_cli_error("--approval-signing-key-file does not match --approval-public-key-file");
        std::process::exit(1);
    }
    let has_durable_authority = viewer_signing_key.is_some() || configured_public_key.is_some();
    let effective_approval_mode =
        resolve_approval_mode(approval_mode, interactive_approval, has_durable_authority);
    if let Err(error) = validate_interactive_approval_authority(
        effective_approval_mode,
        interactive_approval,
        configured_public_key.is_some(),
        viewer_signing_key.is_some(),
    ) {
        print_cli_error(error);
        std::process::exit(1);
    }
    let needs_ephemeral_authority = (effective_approval_mode == ApprovalMode::Prompt
        && interactive_approval)
        || (effective_approval_mode == ApprovalMode::Deny && !has_durable_authority);
    let inline_signing_key = if needs_ephemeral_authority {
        Some(viewer_signing_key.clone().unwrap_or_else(|| {
            sema_workflow::approval::ApprovalSigningKey::generate().unwrap_or_else(|error| {
                print_cli_error(format!("cannot generate interactive approval key: {error}"));
                std::process::exit(1);
            })
        }))
    } else {
        None
    };
    let approval_public_key = if let Some(key) = &viewer_public_key {
        key.clone()
    } else if let Some(key) = &configured_public_key {
        key.clone()
    } else if let Some(key) = &inline_signing_key {
        key.public_key_base64().unwrap_or_else(|error| {
            print_cli_error(format!("cannot derive interactive approval key: {error}"));
            std::process::exit(1);
        })
    } else {
        String::new()
    };
    // An in-memory prompt authority is ephemeral unless it came from the explicit
    // viewer key file. Guidance only promises a resumable authority when the operator
    // supplied one of those durable files.
    let resumable_public_key_file = viewer_signing_key
        .is_none()
        .then_some(approval_public_key_file.as_deref())
        .flatten();
    let resumable_signing_key_file = viewer_signing_key
        .is_some()
        .then_some(approval_signing_key_file.as_deref())
        .flatten();

    // Start the live viewer after loading the host-only signing authority but before
    // evaluation, so the browser can observe the flush-per-event journal immediately.
    // The key remains in server state and is never installed in the interpreter.
    if view {
        let vd = run_dir.clone();
        let viewer_authority = viewer_signing_key.clone().map(|signing_key| {
            workflow_view::ApprovalAuthority::new(
                signing_key,
                approval_actor
                    .clone()
                    .unwrap_or_else(default_approval_actor),
            )
            .unwrap_or_else(|error| {
                print_cli_error(format!("cannot enable viewer approval controls: {error}"));
                std::process::exit(1);
            })
        });
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(async {
                    if let Err(error) = workflow_view::serve_result_with_approval(
                        PathBuf::from(vd),
                        "127.0.0.1",
                        view_port,
                        false,
                        viewer_authority,
                    )
                    .await
                    {
                        print_cli_warning(format!("--view could not start the viewer: {error}"));
                    }
                });
        });
        let url = format!("http://127.0.0.1:{view_port}");
        println!("Live viewer: {url}");
        open_in_browser(&url);
        // Give the listener a moment to bind before the run starts producing events.
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    // Snapshot audit identity before evaluating untrusted workflow code. A workflow may
    // have env-write permission, but cannot rewrite the actor attached to a later host
    // terminal decision.
    let terminal_approval_actor = default_approval_actor();
    let mut active_resume = resume.clone();
    let exit_code = loop {
        let interpreter = build_interpreter(&effective_sandbox);
        for (path, bytes) in &approval_revision.embedded_dependencies {
            interpreter
                .ctx
                .set_embedded_file(path.clone(), bytes.clone());
        }
        let _host_config = sema_workflow::context::install_host_config(
            sema_workflow::context::WorkflowHostConfig {
                runs_root: run_dir.clone(),
                explicit_run_id: active_resume.clone().or_else(|| fresh_run_id.clone()),
                resuming: active_resume.is_some(),
                code_version: code_version.clone(),
                approval_code_version: approval_code_version.clone(),
                args_json: args.clone(),
                approval_public_key: approval_public_key.clone(),
                entry_file: file.clone(),
                workspace_root: workspace_root.to_string_lossy().to_string(),
            },
        );

        // Auto-configure an LLM provider from the environment (mirrors the default run
        // path), so a workflow whose leaves call `llm/*` works without self-configuring.
        // Best-effort: a workflow with no LLM leaves needs no provider, so ignore errors.
        let _ = interpreter.eval_str("(llm/auto-configure)");
        // Evaluate against the same complete static dependency snapshot that was
        // hashed into the approval revision. Macro-generated or runtime-selected
        // imports outside that closure fail closed instead of escaping the binding.
        interpreter.ctx.set_embedded_files_only(true);
        interpreter
            .global_env
            .set(sema_core::intern("*workflow-args*"), args_value.clone());

        let envelope = match interpreter.eval_str_compiled(&content) {
            Ok(envelope) => {
                drain_async_scheduler(&interpreter);
                envelope
            }
            Err(e) => {
                eprint!("Error running workflow {file}: ");
                print_error(&e);
                break 1;
            }
        };
        // Tear down the evaluator (including detached descendants) before any private
        // inline signing key is used at the terminal prompt.
        drop(interpreter);
        let status = envelope
            .as_map_rc()
            .and_then(|m| m.get(&Value::keyword("status")).cloned())
            .and_then(|s| s.as_keyword());
        match status.as_deref() {
            Some("failed") => {
                print_cli_error(format!("workflow failed: {}", pretty_print(&envelope, 80)));
                break 1;
            }
            Some("rejected") => {
                print_cli_error(format!(
                    "workflow approval rejected: {}",
                    pretty_print(&envelope, 80)
                ));
                break 1;
            }
            // The headless-precursor gate (docs/plans/2026-06-24-workflow-mcp-auth.md
            // §3/§5): a declared `:mcp` server had no usable session. Distinct exit
            // code so a CI/orchestrator script can branch on "needs a human to log
            // in" vs. a genuine failure.
            Some("needs-auth") => {
                eprint!("{}", format_needs_auth_guidance(&envelope));
                break 2;
            }
            Some("needs-approval") => {
                let Some(run_id) = approval_envelope_field(&envelope, "run-id") else {
                    print_cli_error("needs-approval envelope is missing :run-id");
                    break 1;
                };
                let Some(approval_id) = approval_envelope_field(&envelope, "approval-id") else {
                    print_cli_error("needs-approval envelope is missing :approval-id");
                    break 1;
                };
                match effective_approval_mode {
                    ApprovalMode::Pause | ApprovalMode::Auto => {
                        eprint!(
                            "{}",
                            format_needs_approval_guidance(
                                &envelope,
                                &run_dir,
                                &file,
                                &args,
                                resumable_public_key_file,
                                resumable_signing_key_file,
                            )
                        );
                        break 3;
                    }
                    ApprovalMode::Deny => {
                        print_cli_error(format!(
                            "workflow reached approval {approval_id}; --approval-mode deny refuses interactive approval"
                        ));
                        break 1;
                    }
                    ApprovalMode::Prompt if !interactive_approval => {
                        print_cli_error(
                            "--approval-mode prompt requires terminal stdin/stderr and is disabled in CI",
                        );
                        eprint!(
                            "{}",
                            format_needs_approval_guidance(
                                &envelope,
                                &run_dir,
                                &file,
                                &args,
                                resumable_public_key_file,
                                resumable_signing_key_file,
                            )
                        );
                        break 3;
                    }
                    ApprovalMode::Prompt => {
                        let action = match prompt_for_workflow_approval(
                            Path::new(&run_dir),
                            &run_id,
                            &approval_id,
                        ) {
                            Ok(action) => action,
                            Err(error) => {
                                print_cli_error(format!("cannot prompt for approval: {error}"));
                                break 3;
                            }
                        };
                        let decision = match action {
                            ApprovalPromptAction::Approve => sema_workflow::approval::decide(
                                Path::new(&run_dir),
                                &run_id,
                                &approval_id,
                                inline_signing_key.as_ref().expect("interactive key exists"),
                                sema_workflow::approval::ApprovalDecisionKind::Approve,
                                terminal_approval_actor.clone(),
                                "terminal-prompt".to_string(),
                                None,
                                None,
                            ),
                            ApprovalPromptAction::Reject(reason) => {
                                sema_workflow::approval::decide(
                                    Path::new(&run_dir),
                                    &run_id,
                                    &approval_id,
                                    inline_signing_key.as_ref().expect("interactive key exists"),
                                    sema_workflow::approval::ApprovalDecisionKind::Reject,
                                    terminal_approval_actor.clone(),
                                    "terminal-prompt".to_string(),
                                    None,
                                    Some(reason),
                                )
                            }
                            ApprovalPromptAction::AlreadyDecided => {
                                active_resume = Some(run_id);
                                continue;
                            }
                            ApprovalPromptAction::LeavePending => {
                                eprint!(
                                    "{}",
                                    format_needs_approval_guidance(
                                        &envelope,
                                        &run_dir,
                                        &file,
                                        &args,
                                        resumable_public_key_file,
                                        resumable_signing_key_file,
                                    )
                                );
                                break 3;
                            }
                        };
                        if let Err(error) = decision {
                            if approval_was_decided(Path::new(&run_dir), &run_id, &approval_id) {
                                eprintln!(
                                    "approval {approval_id} was decided concurrently; resuming with the recorded decision"
                                );
                                active_resume = Some(run_id);
                                continue;
                            }
                            print_cli_error(format!("cannot record approval decision: {error}"));
                            break 1;
                        }
                        active_resume = Some(run_id);
                        continue;
                    }
                }
            }
            _ => break 0,
        }
    };

    // With `--view`, keep the viewer up so the finished run can be inspected.
    if view {
        println!("\nRun complete — viewer live at http://127.0.0.1:{view_port}  (Ctrl-C to stop)");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    std::process::exit(exit_code);
}

/// Render the stderr guidance for a `{:status :needs-auth :auth [{:server :url
/// :persist} …]}` run envelope: terse, terminal-quiet (no banners), one `sema mcp
/// login` line per server, aliases column-aligned. A malformed/missing `:auth`
/// vector degrades to a 0-server header rather than panicking — the exit code
/// alone (2) is still meaningful to a script even if the guidance text is thin.
fn format_needs_auth_guidance(envelope: &Value) -> String {
    let entries: Vec<(String, String)> = envelope
        .as_map_rc()
        .and_then(|m| m.get(&Value::keyword("auth")).cloned())
        .and_then(|a| a.as_list_rc().or_else(|| a.as_vector_rc()))
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    let m = entry.as_map_rc()?;
                    let server = m.get(&Value::keyword("server"))?.as_str()?.to_string();
                    let url = m.get(&Value::keyword("url"))?.as_str()?.to_string();
                    Some((server, url))
                })
                .collect()
        })
        .unwrap_or_default();

    let width = entries.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
    let mut out = format!(
        "run needs authentication for {} MCP server(s):\n",
        entries.len()
    );
    for (server, url) in &entries {
        out.push_str(&format!("  {server:<width$}  sema mcp login {url}\n"));
    }
    out.push_str("then re-run this workflow. (or authenticate from `sema workflow view`)\n");
    out
}

/// Whether `run_workflow_command` should enable inline interactive MCP auth
/// (`sema::workflow_mcp::set_interactive_auth`) for this run: a needs-auth
/// gate logs in right there instead of exiting 2. Pure over its inputs — no
/// direct `IsTerminal`/`env::var` calls here — so this is unit-testable
/// without a real TTY; the caller supplies `std::io::stdin().is_terminal()`,
/// `std::io::stderr().is_terminal()`, `std::env::var("CI").ok()`, and the
/// `--no-auth-prompt` flag.
///
/// Both stdin AND stderr must be TTYs: stdin implies a human is actually at
/// the keyboard to complete a browser (or, if this run were headless, a
/// device-code) flow; stderr is where the "opening browser…" line and any
/// failure reason land, so it must be a place a human will actually see them
/// — an interactive stdin with redirected stderr (e.g. `sema workflow run x
/// 2>log.txt`) is exactly the case that should NOT pop a browser unannounced.
/// `--no-auth-prompt` and a non-empty `CI` both force the headless path
/// unconditionally, regardless of the TTY checks.
fn should_enable_interactive_auth(
    stdin_is_tty: bool,
    stderr_is_tty: bool,
    ci_env: Option<&str>,
    no_auth_prompt: bool,
) -> bool {
    if no_auth_prompt {
        return false;
    }
    if ci_env.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    stdin_is_tty && stderr_is_tty
}

/// Best-effort: open `url` in the default browser via the platform opener. Silent
/// no-op if the opener isn't present (e.g. headless) — the URL is always printed.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Drain any pending async tasks scheduled by a top-level form.
///
/// Retained as a call-site marker; under the unified cooperative runtime the
/// evaluator already drives every spawned task to completion during `eval`
/// (the persistent runtime drains to idle before returning), and any still-
/// detached task is reaped by the interpreter's bounded shutdown on drop. There
/// is no separate scheduler to run, so this is a no-op.
pub(crate) fn drain_async_scheduler(_interpreter: &Interpreter) {}

fn run_notebook_command(command: NotebookCommands) {
    match command {
        NotebookCommands::Serve { file, host, port } => {
            let path = file.map(std::path::PathBuf::from);
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(sema_notebook::serve(path, &host, port));
        }
        NotebookCommands::Run { file, cells } => {
            let path = std::path::Path::new(&file);
            let mut engine = match sema_notebook::Engine::from_file(path) {
                Ok(e) => e,
                Err(e) => {
                    print_cli_error(e);
                    std::process::exit(1);
                }
            };

            // Collect the code cell IDs to evaluate, either specific indices
            // (--cells 1,3,5) or all code cells.
            let cell_ids: Vec<String> = if let Some(cell_spec) = &cells {
                let indices: Vec<usize> = cell_spec
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                engine
                    .notebook
                    .cells
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        if indices.contains(&(i + 1))
                            && c.cell_type == sema_notebook::format::CellType::Code
                        {
                            Some(c.id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                engine
                    .notebook
                    .cells
                    .iter()
                    .filter(|c| c.cell_type == sema_notebook::format::CellType::Code)
                    .map(|c| c.id.clone())
                    .collect()
            };

            let total = cell_ids.len();
            let mut had_error = false;

            for (i, id) in cell_ids.into_iter().enumerate() {
                match engine.eval_cell(&id) {
                    Ok(r) => {
                        if !r.stdout.is_empty() {
                            print!("[{}/{}] (stdout) {}", i + 1, total, r.stdout);
                        }
                        if !r.output.display.is_empty() {
                            println!("[{}/{}] {}", i + 1, total, r.output.display);
                        }
                        if let Some(u) = &r.output.usage {
                            let cost = r
                                .output
                                .cost_usd
                                .map(|c| format!("${c:.4}"))
                                .unwrap_or_else(|| "unpriced".to_string());
                            println!(
                                "[{}/{}] cost: {cost} ({} prompt + {} completion tok)",
                                i + 1,
                                total,
                                u.prompt_tokens,
                                u.completion_tokens
                            );
                        }
                        if r.output.output_type == sema_notebook::format::OutputType::Error {
                            had_error = true;
                        }
                    }
                    Err(e) => {
                        print_cli_error(format!("[{}/{}] {e}", i + 1, total));
                        had_error = true;
                    }
                }
            }

            let session_cost = sema_llm::builtins::session_cost_snapshot();
            if session_cost > 0.0 {
                println!("session cost: ${session_cost:.4}");
            }

            // Save updated outputs back to the file
            if let Err(e) = engine.notebook.save(path) {
                print_cli_warning(format!("could not save: {e}"));
            }

            if had_error {
                std::process::exit(1);
            }
        }
        NotebookCommands::Export {
            file,
            format,
            output,
        } => {
            let path = std::path::Path::new(&file);
            let notebook = match sema_notebook::Notebook::load(path) {
                Ok(nb) => nb,
                Err(e) => {
                    print_cli_error(e);
                    std::process::exit(1);
                }
            };

            let content = match format.as_str() {
                "md" | "markdown" => sema_notebook::render::export_markdown(&notebook),
                other => {
                    print_cli_error(format!(
                        "unknown export format: {other}; supported format: md"
                    ));
                    std::process::exit(1);
                }
            };

            match output {
                Some(out_path) => {
                    if let Err(e) = std::fs::write(&out_path, &content) {
                        print_cli_error(format!("could not write {out_path}: {e}"));
                        std::process::exit(1);
                    }
                    eprintln!("Exported to {out_path}");
                }
                None => print!("{content}"),
            }
        }
        NotebookCommands::New { file, title } => {
            let path = std::path::Path::new(&file);
            let title = title.as_deref().unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled")
            });
            let mut notebook = sema_notebook::Notebook::new(title);
            // Add a starter code cell
            notebook.add_code_cell("; Welcome to your Sema notebook!\n(+ 1 2)");
            if let Err(e) = notebook.save(path) {
                print_cli_error(e);
                std::process::exit(1);
            }
            eprintln!("Created notebook: {file}");
        }
    }
}

fn run_eval(
    use_stdin: bool,
    expr: Option<String>,
    json: bool,
    path: Option<String>,
    timeout_ms: u64,
    sandbox_arg: Option<String>,
    no_llm: bool,
) {
    // Get the program text
    let program = if use_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .unwrap_or_else(|e| {
                if json {
                    let msg = format!("could not read stdin: {e}");
                    print_eval_json(&EvalJsonResult::early_error(&msg));
                } else {
                    print_cli_error(format!("could not read stdin: {e}"));
                }
                std::process::exit(1);
            });
        buf
    } else if let Some(e) = expr {
        e
    } else {
        if json {
            print_eval_json(&EvalJsonResult::early_error(
                "Either --stdin or --expr is required",
            ));
        } else {
            print_cli_error("either --stdin or --expr is required");
        }
        std::process::exit(1);
    };

    // Set up sandbox
    let sandbox = match &sandbox_arg {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            if json {
                let msg = format!("Invalid sandbox: {e}");
                print_eval_json(&EvalJsonResult::early_error(&msg));
            } else {
                print_cli_error(e);
            }
            std::process::exit(1);
        }),
        None => sema_core::Sandbox::allow_all(),
    };

    let interpreter = build_interpreter(&sandbox);

    // Auto-configure LLM unless --no-llm
    if !no_llm {
        let _ = interpreter.eval_str("(llm/auto-configure)");
    }

    // Set file path for import resolution
    if let Some(ref p) = path {
        let file_path = std::path::Path::new(p);
        // Try to canonicalize; fall back to the raw path (supports unsaved/virtual buffers)
        let resolved = file_path
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(p));
        interpreter.ctx.push_file_path(resolved);
    }

    // In JSON mode, capture stdout/stderr from user code by overriding IO functions
    // (same approach as sema-wasm). This prevents print/println/display from
    // corrupting the JSON envelope on real stdout.
    let captured_stdout: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let captured_stderr: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    if json {
        install_capturing_io(&interpreter, &captured_stdout, &captured_stderr);
    }

    // Arm the VM's wall-clock deadline so runaway loops abort with an eval
    // error instead of hanging the caller. Armed after (llm/auto-configure)
    // so setup time is not billed to the user's program; it stays armed
    // through the async drain so spawned tasks are bounded too. A saturating
    // deadline (checked_add → None) means "no timeout", same as 0.
    if timeout_ms > 0 {
        let deadline =
            std::time::Instant::now().checked_add(std::time::Duration::from_millis(timeout_ms));
        interpreter.ctx.set_eval_deadline(deadline);
    }

    let start = std::time::Instant::now();
    let result = interpreter.eval_str_compiled(&program);
    if result.is_ok() {
        drain_async_scheduler(&interpreter);
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let stdout_text = captured_stdout.borrow();
    let stderr_text = captured_stderr.borrow();

    match result {
        Ok(val) => {
            if json {
                let val_str = if val.is_nil() {
                    None
                } else {
                    Some(pretty_print(&val, 120))
                };
                print_eval_json(&EvalJsonResult {
                    ok: true,
                    value: val_str.as_deref(),
                    stdout: &stdout_text,
                    stderr: &stderr_text,
                    error_msg: None,
                    error_hint: None,
                    error_line: None,
                    error_col: None,
                    elapsed_ms,
                });
            } else if !val.is_nil() {
                println!("{}", pretty_print(&val, 120));
            }
        }
        Err(e) => {
            let inner = e.inner();
            let msg = e.user_message();
            let hint = e.hint().map(|s| s.to_string());
            // Extract line+col from Reader span or first stack trace frame
            let (line, col) = match inner {
                SemaError::Reader { span, .. } => (Some(span.line), Some(span.col)),
                _ => e
                    .stack_trace()
                    .and_then(|t| t.0.first())
                    .and_then(|f| f.span.as_ref())
                    .map(|s| (Some(s.line), Some(s.col)))
                    .unwrap_or((None, None)),
            };
            if json {
                print_eval_json(&EvalJsonResult {
                    ok: false,
                    value: None,
                    stdout: &stdout_text,
                    stderr: &stderr_text,
                    error_msg: Some(&msg),
                    error_hint: hint.as_deref(),
                    error_line: line,
                    error_col: col,
                    elapsed_ms,
                });
            } else {
                print_error(&e);
                std::process::exit(1);
            }
        }
    }
}

/// Override display/print/println/pprint/newline/print-error/println-error to write
/// to in-memory buffers instead of real stdout/stderr. This prevents user code output
/// from corrupting the JSON envelope in `sema eval --json` mode.
fn install_capturing_io(
    interpreter: &Interpreter,
    stdout_buf: &Rc<RefCell<String>>,
    stderr_buf: &Rc<RefCell<String>>,
) {
    use sema_core::{intern, NativeFn, Value};
    let env = &interpreter.global_env;

    // Helper: register a simple native fn that captures to a buffer
    macro_rules! capture_fn {
        ($name:expr, $buf:expr, $newline:expr, $raw:expr) => {{
            let buf = $buf.clone();
            env.set(
                intern($name),
                Value::native_fn(NativeFn::simple($name, move |args| {
                    let mut out = buf.borrow_mut();
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push(' ');
                        }
                        if $raw {
                            out.push_str(&format!("{arg}"));
                        } else {
                            match arg.as_str() {
                                Some(s) => out.push_str(s),
                                None => out.push_str(&format!("{arg}")),
                            }
                        }
                    }
                    if $newline {
                        out.push('\n');
                    }
                    Ok(Value::nil())
                })),
            );
        }};
    }

    // stdout-targeting functions
    capture_fn!("display", stdout_buf, false, false);
    capture_fn!("print", stdout_buf, false, true);
    capture_fn!("println", stdout_buf, true, false);
    capture_fn!("newline", stdout_buf, true, false);

    // pprint needs special handling (uses pretty_print)
    let pprint_buf = stdout_buf.clone();
    env.set(
        intern("pprint"),
        Value::native_fn(NativeFn::simple("pprint", move |args| {
            sema_core::check_arity!(args, "pprint", 1);
            let mut out = pprint_buf.borrow_mut();
            out.push_str(&sema_core::pretty_print(&args[0], 80));
            out.push('\n');
            Ok(Value::nil())
        })),
    );

    // stderr-targeting functions
    capture_fn!("print-error", stderr_buf, false, false);
    capture_fn!("println-error", stderr_buf, true, false);
}

struct EvalJsonResult<'a> {
    ok: bool,
    value: Option<&'a str>,
    stdout: &'a str,
    stderr: &'a str,
    error_msg: Option<&'a str>,
    error_hint: Option<&'a str>,
    error_line: Option<usize>,
    error_col: Option<usize>,
    elapsed_ms: u64,
}

impl<'a> EvalJsonResult<'a> {
    /// An early-failure envelope used before evaluation runs: `ok:false`, no
    /// value, no captured output, no source span, zero elapsed time. The only
    /// thing that varies between sites is the error message.
    fn early_error(msg: &'a str) -> Self {
        Self {
            ok: false,
            value: None,
            stdout: "",
            stderr: "",
            error_msg: Some(msg),
            error_hint: None,
            error_line: None,
            error_col: None,
            elapsed_ms: 0,
        }
    }
}

fn print_eval_json(r: &EvalJsonResult) {
    let result = serde_json::json!({
        "ok": r.ok,
        "value": r.value,
        "stdout": r.stdout,
        "stderr": r.stderr,
        "error": r.error_msg.map(|msg| {
            let mut err = serde_json::json!({ "message": msg });
            if let Some(hint) = r.error_hint {
                err["hint"] = serde_json::json!(hint);
            }
            if let Some(line) = r.error_line {
                err["line"] = serde_json::json!(line);
            }
            if let Some(col) = r.error_col {
                err["col"] = serde_json::json!(col);
            }
            err
        }),
        "elapsedMs": r.elapsed_ms,
    });
    println!("{}", serde_json::to_string(&result).unwrap());
}

fn run_compile(file: &str, output: Option<&str>) {
    let path = std::path::Path::new(file);
    let source = match read_source_file(path) {
        Ok(s) => s,
        Err(msg) => {
            print_cli_error(msg);
            std::process::exit(1);
        }
    };

    // Compute source hash (CRC-32)
    let source_hash = crc32fast::hash(source.as_bytes());

    // Use Interpreter for macro expansion before compilation
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let result = match interpreter.compile_to_bytecode(&source) {
        Ok(r) => r,
        Err(e) => {
            print_cli_error(format!("compilation failed: {}", e.format_plain()));
            std::process::exit(1);
        }
    };

    // Serialize
    let bytes = match sema_vm::serialize_to_bytes(&result, source_hash) {
        Ok(b) => b,
        Err(e) => {
            print_cli_error(format!("serialization failed: {}", e.format_plain()));
            std::process::exit(1);
        }
    };

    // Write output
    let out_path = match output {
        Some(o) => std::path::PathBuf::from(o),
        None => path.with_extension("semac"),
    };
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        print_cli_error(format!("could not write {}: {e}", out_path.display()));
        std::process::exit(1);
    }
}

fn try_run_embedded() -> Option<i32> {
    let exe_path = std::env::current_exe().ok()?;

    // Try named section first (macOS Mach-O / Windows PE via libsui),
    // fall back to trailer scan (Linux ELF raw append).
    let archive_data = if let Ok(Some(data)) = libsui::find_section("semaexec") {
        data.to_vec()
    } else if archive::has_embedded_archive(&exe_path).ok()? {
        match std::fs::read(&exe_path) {
            Ok(data) => {
                let len = data.len();
                let trailer = &data[len - 16..];
                let archive_size = u64::from_le_bytes(trailer[0..8].try_into().unwrap()) as usize;
                data[len - 16 - archive_size..len - 16].to_vec()
            }
            Err(_) => return None,
        }
    } else {
        return None;
    };

    let arch = match archive::deserialize_archive_from_bytes(&archive_data) {
        Ok(a) => a,
        Err(e) => {
            print_cli_error(format!("could not load embedded archive: {e}"));
            return Some(1);
        }
    };

    let entry_point = arch
        .metadata
        .get("entry-point")
        .and_then(|v| std::str::from_utf8(v).ok())
        .unwrap_or("__main__.semac")
        .to_string();

    let bytecode = match arch.files.get(&entry_point) {
        Some(b) => b.clone(),
        None => {
            print_cli_error(format!(
                "entry point '{entry_point}' was not found in the embedded archive"
            ));
            return Some(1);
        }
    };

    sema_core::vfs::init_vfs(arch.files);

    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let _ = interpreter.eval_str("(llm/auto-configure)");

    let args: Vec<String> = std::env::args().collect();
    let is_mcp = args
        .iter()
        .any(|arg| arg == "--mcp" || arg.starts_with("--mcp="));

    if is_mcp {
        let mut include = None;
        let mut exclude = None;
        let mut timeout_ms = None;
        for window in args.windows(2) {
            if window[0] == "--include" {
                include = Some(window[1].clone());
            } else if window[0] == "--exclude" {
                exclude = Some(window[1].clone());
            } else if window[0] == "--timeout-ms" {
                timeout_ms = window[1].trim().parse::<u64>().ok();
            }
        }
        for arg in &args {
            if let Some(rest) = arg.strip_prefix("--include=") {
                include = Some(rest.to_string());
            } else if let Some(rest) = arg.strip_prefix("--exclude=") {
                exclude = Some(rest.to_string());
            } else if let Some(rest) = arg.strip_prefix("--timeout-ms=") {
                timeout_ms = rest.trim().parse::<u64>().ok();
            }
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
        let tool_timeout = resolve_mcp_tool_timeout(timeout_ms);

        if let Err(e) = run_bytecode_bytes(&interpreter, &bytecode) {
            print_error(&e);
            return Some(1);
        }

        // Same no-ambient-runtime rule as the CLI mcp arm (llm/* + io_block_on).
        if let Err(e) =
            sema_mcp::run_mcp_server_sync(interpreter, inc_tools, exc_tools, tool_timeout)
        {
            print_cli_error(format!("MCP server failed: {e}"));
            std::process::exit(1);
        }
        Some(0)
    } else {
        match run_bytecode_bytes(&interpreter, &bytecode) {
            Ok(_) => Some(0),
            Err(e) => {
                print_error(&e);
                Some(1)
            }
        }
    }
}

/// Expand a leading `~` to the user's home directory. Shells do not tilde-expand
/// inside `--output=~/x` (the `~` follows `=`), so without this the path would be
/// created literally as a directory named `~` in the cwd — a deletion hazard.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    let home = || std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
    if path == "~" {
        if let Ok(h) = home() {
            return std::path::PathBuf::from(h);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(h) = home() {
            return std::path::Path::new(&h).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// The default output filename for a build: source stem, plus `.exe` for Windows
/// targets (or a host build on Windows).
fn default_output_name(source: &std::path::Path, target: Option<&str>) -> String {
    let stem = source
        .file_stem()
        .unwrap_or(source.as_os_str())
        .to_string_lossy();
    let needs_exe = target
        .and_then(|t| cross_compile::resolve_target(t).ok())
        .is_some_and(cross_compile::is_windows_target)
        || (target.is_none() && cfg!(windows));
    if needs_exe {
        format!("{stem}.exe")
    } else {
        stem.into_owned()
    }
}

/// Resolve `--output` to the actual file path a single-target build writes.
/// A path that IS a directory (or ends in a separator) means "default filename
/// inside this directory"; anything else is the file path itself.
fn resolve_output_path(
    output: Option<&str>,
    source: &std::path::Path,
    target: Option<&str>,
) -> std::path::PathBuf {
    match output {
        None => std::path::PathBuf::from(default_output_name(source, target)),
        Some(o) => {
            let p = expand_tilde(o);
            if p.is_dir() || o.ends_with('/') || o.ends_with(std::path::MAIN_SEPARATOR) {
                p.join(default_output_name(source, target))
            } else {
                p
            }
        }
    }
}

/// Plan the per-target output paths for `--target all`, honoring `--output`:
/// a directory output gets `<dir>/<stem>-<target>[.exe]`; a file-ish output is
/// used as the base name, `<base>-<target>[.exe]` (a trailing `.exe` on the base
/// is dropped first); no output means `<stem>-<target>[.exe]` in the cwd.
fn plan_all_target_outputs(
    output: Option<&str>,
    source: &std::path::Path,
) -> Vec<(&'static str, std::path::PathBuf)> {
    let stem = source
        .file_stem()
        .unwrap_or(source.as_os_str())
        .to_string_lossy()
        .into_owned();
    let (dir, base): (std::path::PathBuf, String) = match output {
        None => (std::path::PathBuf::new(), stem),
        Some(o) => {
            let p = expand_tilde(o);
            if p.is_dir() || o.ends_with('/') || o.ends_with(std::path::MAIN_SEPARATOR) {
                (p, stem)
            } else {
                let base = p
                    .file_name()
                    .map(|f| {
                        let f = f.to_string_lossy();
                        f.strip_suffix(".exe").unwrap_or(&f).to_string()
                    })
                    .unwrap_or(stem);
                (
                    p.parent().map(|d| d.to_path_buf()).unwrap_or_default(),
                    base,
                )
            }
        }
    };
    cross_compile::SUPPORTED_TARGETS
        .iter()
        .map(|&t| {
            let ext = if cross_compile::is_windows_target(t) {
                ".exe"
            } else {
                ""
            };
            (t, dir.join(format!("{base}-{t}{ext}")))
        })
        .collect()
}

/// Output-mode flags for `sema build`, threaded through the pipeline.
#[derive(Clone, Copy, Default)]
struct BuildOutputOpts {
    verbose: bool,
    json: bool,
}

/// The target-independent build product — one archive, embedded into every
/// per-target executable.
struct BuildArchive {
    files_count: usize,
    archive_bytes: Vec<u8>,
}

/// One successfully written artifact (native executable or web .vfs archive).
struct BuiltArtifact {
    /// Resolved target triple, or "web".
    target: String,
    /// Absolute output path.
    path: std::path::PathBuf,
    bytes: u64,
    /// How the runtime was obtained: "host" | "cached" | "downloaded" | "custom",
    /// or None for web (no runtime is embedded).
    runtime: Option<&'static str>,
    duration: std::time::Duration,
}

/// `29289346` → `27.9 MB` (base-1024, one decimal above bytes).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn human_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// Split a target triple into user-facing (os, arch) — `aarch64-apple-darwin`
/// → ("macos", "arm64"). Unknown triples fall back to the raw string.
fn triple_os_arch(target: &str) -> (&'static str, String) {
    if target == "web" {
        return ("web", "-".to_string());
    }
    let arch = match target.split('-').next().unwrap_or(target) {
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    };
    let os = if target.contains("apple-darwin") {
        "macos"
    } else if target.contains("linux") {
        "linux"
    } else if target.contains("windows") {
        "windows"
    } else {
        "?"
    };
    (os, arch)
}

fn sha256_hex_of_file(path: &std::path::Path) -> Option<String> {
    use sha2::Digest;
    let data = std::fs::read(path).ok()?;
    let hash = sha2::Sha256::digest(&data);
    Some(hash.iter().map(|b| format!("{b:02x}")).collect())
}

/// Refuse to clobber the source file itself (`-o hello.sema` would otherwise
/// silently replace the program with its own binary).
fn check_output_not_source(
    source: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<(), String> {
    let a = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let b = std::fs::canonicalize(output_path).unwrap_or_else(|_| output_path.to_path_buf());
    if a == b {
        return Err(format!(
            "Output path would overwrite the source file '{}'.\n  Hint: use `-o <output>` to specify a different output path, or rename your source file to use a .sema extension.",
            source.display()
        ));
    }
    Ok(())
}

/// Steps 1–4: compile → trace imports → collect assets → serialize archive.
/// Fully target-independent; `--target all` runs this ONCE. Prints the per-step
/// detail only in verbose mode, plus a one-line completion note always (stderr).
fn build_archive(
    file: &str,
    includes: &[String],
    opts: BuildOutputOpts,
) -> Result<BuildArchive, String> {
    let path = std::path::Path::new(file);
    let source = read_source_file(path)?;

    if opts.verbose {
        eprintln!("[1/4] Compiling {file}...");
    }

    let source_hash = crc32fast::hash(source.as_bytes());
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let result = interpreter
        .compile_to_bytecode(&source)
        .map_err(|e| format!("compile failed: {}", e.format_plain()))?;
    let bytecode = sema_vm::serialize_to_bytes(&result, source_hash)
        .map_err(|e| format!("serialization failed: {}", e.format_plain()))?;

    if opts.verbose {
        eprintln!("[2/4] Tracing imports...");
    }
    let imports =
        import_tracer::trace_imports(path).map_err(|e| format!("tracing imports: {e}"))?;

    if opts.verbose {
        eprintln!("[3/4] Collecting assets...");
    }
    let mut files = std::collections::HashMap::new();
    files.insert("__main__.semac".to_string(), bytecode);

    for (rel_path, contents) in &imports {
        if let Err(e) = sema_core::vfs::validate_vfs_path(rel_path) {
            print_cli_warning(format!("skipping import with invalid VFS path: {e}"));
            continue;
        }
        files.insert(rel_path.clone(), contents.clone());
    }

    for include in includes {
        let inc_path = std::path::Path::new(include);
        if inc_path.is_dir() {
            let base = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            collect_directory_files(inc_path, &base, &mut files);
        } else if inc_path.is_file() {
            let rel = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            if let Err(e) = sema_core::vfs::validate_vfs_path(&rel) {
                print_cli_warning(format!("skipping {include}: {e}"));
                continue;
            }
            match std::fs::read(inc_path) {
                Ok(data) => {
                    files.insert(rel, data);
                }
                Err(e) => {
                    print_cli_warning(format!("cannot read {include}: {e}"));
                }
            }
        } else {
            print_cli_warning(format!("--include path not found: {include}"));
        }
    }

    if opts.verbose {
        eprintln!(
            "[4/4] Building archive ({} file{})...",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "sema-version".to_string(),
        env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
    );
    metadata.insert(
        "build-timestamp".to_string(),
        build_timestamp().into_bytes(),
    );
    metadata.insert("entry-point".to_string(), b"__main__.semac".to_vec());

    let canonical_root = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    metadata.insert(
        "build-root".to_string(),
        canonical_root.to_string_lossy().into_owned().into_bytes(),
    );

    let archive_bytes = archive::serialize_archive(&metadata, &files);
    eprintln!(
        "Compiled {file} → archive ({} file{}, {})",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        human_size(archive_bytes.len() as u64)
    );

    Ok(BuildArchive {
        files_count: files.len(),
        archive_bytes,
    })
}

/// Step 5: resolve the runtime for one target and write the executable.
fn write_target_executable(
    source: &std::path::Path,
    archive: &BuildArchive,
    output_path: &std::path::Path,
    runtime: Option<&str>,
    target: Option<&str>,
    no_cache: bool,
    opts: BuildOutputOpts,
) -> Result<BuiltArtifact, String> {
    let started = std::time::Instant::now();
    check_output_not_source(source, output_path)?;
    probe_output_writable(output_path)?;

    let resolved_target = target.and_then(|t| cross_compile::resolve_target(t).ok());

    let (runtime_path, runtime_source): (std::path::PathBuf, &'static str) = if let Some(r) =
        runtime
    {
        // Validate runtime binary format against target if both are specified
        if let Some(resolved) = resolved_target {
            let runtime_bytes =
                std::fs::read(r).map_err(|e| format!("cannot read --runtime file '{}': {e}", r))?;
            let detected = cross_compile::detect_binary_format(&runtime_bytes);
            let expected = cross_compile::expected_format(resolved);
            if let Some(det) = detected {
                if det != expected {
                    return Err(format!(
                        "Runtime binary format mismatch: {resolved} expects {expected} but --runtime file is {det}\n  Hint: provide a {expected} binary built for {resolved}, or omit --runtime to download automatically."
                    ));
                }
            }
        }
        (std::path::PathBuf::from(r), "custom")
    } else if let Some(t) = target {
        let resolved = cross_compile::resolve_target(t).map_err(|e| e.to_string())?;
        if cross_compile::is_host_target(resolved) {
            if opts.verbose {
                eprintln!("  Target {resolved} matches host — using local runtime (no download)");
            }
            let exe = std::env::current_exe()
                .map_err(|e| format!("cannot determine current executable path: {e}"))?;
            (exe, "host")
        } else {
            let (path, fetch) = cross_compile::ensure_runtime(resolved, no_cache, opts.verbose)
                .map_err(|e| e.to_string())?;
            let label = match fetch {
                cross_compile::RuntimeFetch::Cached => "cached",
                cross_compile::RuntimeFetch::Downloaded => "downloaded",
            };
            (path, label)
        }
    } else {
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot determine current executable path: {e}"))?;
        (exe, "host")
    };

    write_executable_platform(&runtime_path, output_path, &archive.archive_bytes)
        .map_err(|e| format!("writing executable: {e}"))?;

    let bytes = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
    let abs = std::path::absolute(output_path).unwrap_or_else(|_| output_path.to_path_buf());
    Ok(BuiltArtifact {
        target: resolved_target
            .unwrap_or_else(cross_compile::host_target)
            .to_string(),
        path: abs,
        bytes,
        runtime: Some(runtime_source),
        duration: started.elapsed(),
    })
}

/// Print the machine-readable manifest for any build (single, all, or web).
fn print_build_json(
    source: &str,
    bundled_files: usize,
    archive_bytes: usize,
    artifacts: &[BuiltArtifact],
    failures: &[(String, String)],
    elapsed: std::time::Duration,
) {
    let mut targets: Vec<serde_json::Value> = Vec::new();
    for a in artifacts {
        let (os, arch) = triple_os_arch(&a.target);
        targets.push(serde_json::json!({
            "target": a.target,
            "os": os,
            "arch": arch,
            "path": a.path,
            "bytes": a.bytes,
            "sha256": sha256_hex_of_file(&a.path),
            "runtime": a.runtime,
            "ok": true,
            "error": null,
        }));
    }
    for (t, e) in failures {
        let (os, arch) = triple_os_arch(t);
        targets.push(serde_json::json!({
            "target": t,
            "os": os,
            "arch": arch,
            "path": null,
            "bytes": null,
            "sha256": null,
            "runtime": null,
            "ok": false,
            "error": e,
        }));
    }
    let manifest = serde_json::json!({
        "source": std::path::absolute(source).unwrap_or_else(|_| source.into()),
        "bundled_files": bundled_files,
        "archive_bytes": archive_bytes,
        "duration_ms": elapsed.as_millis() as u64,
        "targets": targets,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string())
    );
}

/// Render the aligned human summary table for `--target all`.
fn render_build_summary(
    artifacts: &[BuiltArtifact],
    failures: &[(String, String)],
    elapsed: std::time::Duration,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let total = artifacts.len() + failures.len();
    let _ = writeln!(
        out,
        "Built {}/{} target{} in {}:",
        artifacts.len(),
        total,
        if total == 1 { "" } else { "s" },
        human_duration(elapsed)
    );
    let _ = writeln!(out);
    // rows: (os, arch, size-or-FAILED, path-or-error)
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    for a in artifacts {
        let (os, arch) = triple_os_arch(&a.target);
        rows.push((
            os.to_string(),
            arch,
            human_size(a.bytes),
            a.path.display().to_string(),
        ));
    }
    for (t, e) in failures {
        let (os, arch) = triple_os_arch(t);
        let first = e.lines().next().unwrap_or("build failed").to_string();
        rows.push((os.to_string(), arch, "FAILED".to_string(), first));
    }
    let w_os = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let w_arch = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
    let w_size = rows.iter().map(|r| r.2.len()).max().unwrap_or(0);
    for (os, arch, size, path) in rows {
        let _ = writeln!(
            out,
            "  {os:<w_os$}  {arch:<w_arch$}  {size:>w_size$}   {path}"
        );
    }
    out
}

/// Build every supported native target from one shared archive.
fn run_build_all(
    file: &str,
    output: Option<&str>,
    includes: &[String],
    runtime: Option<&str>,
    no_cache: bool,
    opts: BuildOutputOpts,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    if runtime.is_some() {
        // clap's conflicts_with should prevent this; belt and braces.
        return Err("--runtime cannot be combined with --target all".to_string());
    }
    let source = std::path::Path::new(file);
    let archive = build_archive(file, includes, opts)?;
    let plans = plan_all_target_outputs(output, source);
    let name_w = plans.iter().map(|(t, _)| t.len()).max().unwrap_or(0);

    let mut artifacts: Vec<BuiltArtifact> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    for (t, target_output) in &plans {
        match write_target_executable(
            source,
            &archive,
            target_output,
            None,
            Some(t),
            no_cache,
            opts,
        ) {
            Ok(a) => {
                eprintln!(
                    "  {t:<name_w$}  ✓ {:>6}  ({} runtime)",
                    human_duration(a.duration),
                    a.runtime.unwrap_or("host"),
                );
                artifacts.push(a);
            }
            Err(e) => {
                let first = e.lines().next().unwrap_or("build failed");
                eprintln!("  {t:<name_w$}  ✗ {first}");
                failures.push((t.to_string(), e));
            }
        }
    }

    eprintln!();
    if opts.json {
        print_build_json(
            file,
            archive.files_count,
            archive.archive_bytes.len(),
            &artifacts,
            &failures,
            started.elapsed(),
        );
    } else {
        print!(
            "{}",
            render_build_summary(&artifacts, &failures, started.elapsed())
        );
    }

    if !failures.is_empty() {
        let failed: Vec<&str> = failures.iter().map(|(t, _)| t.as_str()).collect();
        return Err(format!(
            "failed to build for {} target(s): {}\n  Hint: re-run a single target for details: `sema build --target <target> {}`\n  Hint: use `--runtime /path/to/sema` if downloads fail, or install a released version of sema.",
            failed.len(),
            failed.join(", "),
            file
        ));
    }
    Ok(())
}

fn run_build(
    file: &str,
    output: Option<&str>,
    includes: &[String],
    runtime: Option<&str>,
    target: Option<&str>,
    no_cache: bool,
    opts: BuildOutputOpts,
) -> Result<(), String> {
    if target == Some("all") {
        return run_build_all(file, output, includes, runtime, no_cache, opts);
    }
    if target == Some("web") {
        return run_build_web(file, output, includes, opts);
    }

    let started = std::time::Instant::now();
    let path = std::path::Path::new(file);

    // Pre-flight before any compilation: resolve the output path, refuse to
    // clobber the source, and probe the parent for writability (creating missing
    // directories). Avoids "failed at the last step" after a full compile.
    let output_path = resolve_output_path(output, path, target);
    check_output_not_source(path, &output_path)?;
    probe_output_writable(&output_path)?;

    let archive = build_archive(file, includes, opts)?;
    let artifact = write_target_executable(
        path,
        &archive,
        &output_path,
        runtime,
        target,
        no_cache,
        opts,
    )?;

    if opts.json {
        print_build_json(
            file,
            archive.files_count,
            archive.archive_bytes.len(),
            std::slice::from_ref(&artifact),
            &[],
            started.elapsed(),
        );
    } else {
        println!(
            "Built {} ({}, {} bundled file{}) for {} in {}",
            artifact.path.display(),
            human_size(artifact.bytes),
            archive.files_count,
            if archive.files_count == 1 { "" } else { "s" },
            artifact.target,
            human_duration(started.elapsed())
        );
    }
    Ok(())
}

fn compile_source_to_bytecode(source: &str) -> Result<Vec<u8>, String> {
    let source_hash = crc32fast::hash(source.as_bytes());
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = Interpreter::new_with_sandbox(&sandbox);
    interpreter
        .eval_str_in_global(include_str!("web_prelude.sema"))
        .map_err(|e| format!("web prelude failed: {}", e.format_plain()))?;
    let result = interpreter
        .compile_to_bytecode(source)
        .map_err(|e| format!("compile failed: {}", e.format_plain()))?;
    sema_vm::serialize_to_bytes(&result, source_hash)
        .map_err(|e| format!("serialization failed: {}", e.format_plain()))
}

fn should_compile_traced_import(rel_path: &str) -> bool {
    rel_path.ends_with(".sema") || sema_core::resolve::is_package_import(rel_path)
}

fn web_output_path(input: &std::path::Path, output: Option<&str>) -> std::path::PathBuf {
    let default_name = format!(
        "{}.vfs",
        input
            .file_stem()
            .unwrap_or(input.as_os_str())
            .to_string_lossy()
    );

    match output {
        Some(raw) => {
            let path = expand_tilde(raw);
            if path.is_dir() || raw.ends_with(std::path::MAIN_SEPARATOR) {
                path.join(default_name)
            } else if path.extension().is_none() {
                path.with_extension("vfs")
            } else {
                path
            }
        }
        None => std::path::PathBuf::from(default_name),
    }
}

/// Compile an entry `.sema` plus its traced imports (and any `includes`) into a
/// web `.vfs` archive. Returns the archive bytes and the number of traced
/// imports (0 = single-file). Shared by `sema build --target web` and the
/// `sema web` dev server, which builds an archive on the fly for multi-file apps.
pub(crate) fn build_web_archive(
    path: &std::path::Path,
    includes: &[String],
    opts: BuildOutputOpts,
) -> Result<(Vec<u8>, usize), String> {
    let source =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if opts.verbose {
        eprintln!("[1/4] Compiling {} (web prelude)...", path.display());
    }
    let entry_bytecode = compile_source_to_bytecode(&source)?;

    if opts.verbose {
        eprintln!("[2/4] Tracing imports...");
    }
    let imports =
        import_tracer::trace_imports(path).map_err(|e| format!("tracing imports: {e}"))?;

    let mut files = std::collections::HashMap::new();
    files.insert("__main__.semac".to_string(), entry_bytecode);

    for (rel_path, contents) in &imports {
        if let Err(e) = sema_core::vfs::validate_vfs_path(rel_path) {
            print_cli_warning(format!("skipping import with invalid VFS path: {e}"));
            continue;
        }

        let bundled = if should_compile_traced_import(rel_path) {
            let import_source = String::from_utf8(contents.clone()).map_err(|e| {
                format!("compile error in {rel_path}: import is not valid UTF-8: {e}")
            })?;
            compile_source_to_bytecode(&import_source).map_err(|e| format!("{e} in {rel_path}"))?
        } else {
            contents.clone()
        };

        files.insert(rel_path.clone(), bundled);
    }

    if opts.verbose {
        eprintln!("[3/4] Collecting assets...");
    }
    for include in includes {
        let inc_path = std::path::Path::new(include);
        if inc_path.is_dir() {
            let base = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            collect_directory_files(inc_path, &base, &mut files);
        } else if inc_path.is_file() {
            let rel = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            if let Err(e) = sema_core::vfs::validate_vfs_path(&rel) {
                print_cli_warning(format!("skipping {include}: {e}"));
                continue;
            }
            match std::fs::read(inc_path) {
                Ok(data) => {
                    files.insert(rel, data);
                }
                Err(e) => {
                    print_cli_warning(format!("cannot read {include}: {e}"));
                }
            }
        } else {
            print_cli_warning(format!("--include path not found: {include}"));
        }
    }

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "sema-version".to_string(),
        env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
    );
    metadata.insert(
        "build-timestamp".to_string(),
        build_timestamp().into_bytes(),
    );
    metadata.insert("entry-point".to_string(), b"__main__.semac".to_vec());
    metadata.insert("build-target".to_string(), b"web".to_vec());

    let canonical_root = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    metadata.insert(
        "build-root".to_string(),
        canonical_root.to_string_lossy().into_owned().into_bytes(),
    );

    if opts.verbose {
        eprintln!(
            "[4/4] Building archive ({} file{})...",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }
    let files_count = files.len();
    Ok((archive::serialize_archive(&metadata, &files), files_count))
}

fn run_build_web(
    file: &str,
    output: Option<&str>,
    includes: &[String],
    opts: BuildOutputOpts,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let path = std::path::Path::new(file);
    if !path.exists() {
        return Err(format!("source file not found: {file}"));
    }

    let (archive_bytes, files_count) = build_web_archive(path, includes, opts)?;
    eprintln!(
        "Compiled {file} → web archive ({} file{}, {})",
        files_count,
        if files_count == 1 { "" } else { "s" },
        human_size(archive_bytes.len() as u64)
    );

    let output_path = web_output_path(path, output);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating output directory {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(&output_path, &archive_bytes)
        .map_err(|e| format!("writing {}: {e}", output_path.display()))?;

    let abs = std::path::absolute(&output_path).unwrap_or_else(|_| output_path.clone());
    if opts.json {
        let artifact = BuiltArtifact {
            target: "web".to_string(),
            path: abs,
            bytes: archive_bytes.len() as u64,
            runtime: None,
            duration: started.elapsed(),
        };
        print_build_json(
            file,
            files_count,
            archive_bytes.len(),
            std::slice::from_ref(&artifact),
            &[],
            started.elapsed(),
        );
    } else {
        println!(
            "Built {} ({}, {} bundled file{}) for web in {}",
            abs.display(),
            human_size(archive_bytes.len() as u64),
            files_count,
            if files_count == 1 { "" } else { "s" },
            human_duration(started.elapsed())
        );
    }

    Ok(())
}

/// Probe whether we can write to the directory that will hold `output_path`.
///
/// Creates and immediately deletes a tiny probe file in the parent directory.
/// Returns a clear error before the build commits to any work if the directory
/// doesn't exist or denies writes (e.g. /readonly/sema, /no/such/dir/sema).
fn probe_output_writable(output_path: &Path) -> Result<(), String> {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    if !parent.exists() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create output directory {}: {e}", parent.display()))?;
    }
    let probe_name = format!(
        ".sema-build-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let probe = parent.join(probe_name);
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::PermissionDenied => Err(format!(
                "permission denied writing to {} (for output {})",
                parent.display(),
                output_path.display()
            )),
            std::io::ErrorKind::NotFound => Err(format!(
                "output directory does not exist: {}",
                parent.display()
            )),
            _ => Err(format!(
                "cannot write to {}: {}",
                parent.display(),
                strip_os_error(&e.to_string())
            )),
        },
    }
}

/// Strip trailing " (os error N)" from a system error string for nicer output.
fn strip_os_error(s: &str) -> String {
    if let Some(idx) = s.rfind(" (os error ") {
        if s.ends_with(')') {
            return s[..idx].to_string();
        }
    }
    s.to_string()
}

/// Add a `VERSIONINFO` resource to a `sema build` Windows executable so Explorer's
/// Details tab shows the program name and the Sema runtime version. The resource
/// directory already contains the payload + icons written by libsui; editpe rebuilds
/// it in place, preserving them.
fn set_windows_version_info(
    pe_bytes: Vec<u8>,
    output_path: &std::path::Path,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use editpe::types::{VersionU16, VersionU32};

    let program = output_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "program".to_string());
    let sema_version = env!("CARGO_PKG_VERSION");
    let (maj, min, pat) = {
        let mut it = sema_version
            .split('.')
            .map(|p| p.parse::<u16>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    };
    // VS_FIXEDFILEINFO packs each version as two dwords: MS = major<<16|minor,
    // LS = patch<<16|build.
    let version = VersionU32 {
        major: ((maj as u32) << 16) | min as u32,
        minor: (pat as u32) << 16,
    };

    let mut info = editpe::VersionInfo::default();
    info.info.file_version = version;
    info.info.product_version = version;
    info.info.file_os = 0x0004_0004; // VOS_NT_WINDOWS32
    info.info.file_type = 0x1; // VFT_APP

    // en-US (0x0409), Unicode codepage (0x04B0) — key format Windows expects.
    let mut table = editpe::VersionStringTable {
        key: "040904b0".to_string(),
        strings: Default::default(),
    };
    for (k, v) in [
        ("ProductName", program.clone()),
        ("FileDescription", format!("{program} (built with Sema)")),
        ("FileVersion", sema_version.to_string()),
        ("ProductVersion", sema_version.to_string()),
        ("OriginalFilename", format!("{program}.exe")),
    ] {
        table.strings.insert(k.to_string(), v);
    }
    info.strings.push(table);
    info.vars.push(VersionU16 {
        major: 0x0409,
        minor: 0x04B0,
    });

    let mut image = editpe::Image::parse(&pe_bytes[..])?;
    let mut dir = image.resource_directory().cloned().unwrap_or_default();
    dir.set_version_info(&info)?;
    image.set_resource_directory(dir)?;
    Ok(image.data().to_vec())
}

/// Write the executable using format-aware injection.
///
/// Detects the binary format at runtime (not compile-time) so that
/// cross-compilation works: e.g. injecting into an ELF binary from macOS.
/// Note: libsui uses pure Rust for Mach-O ad-hoc signing (sha2 + object crate),
/// so cross-injecting Mach-O from Linux works without macOS tools.
fn write_executable_platform(
    runtime_path: &std::path::Path,
    output_path: &std::path::Path,
    archive_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = std::fs::read(runtime_path)?;

    let format = cross_compile::detect_binary_format(&runtime).ok_or_else(|| {
        if runtime.len() < 4 {
            "runtime binary too small to detect format".to_string()
        } else if runtime[..4].iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
            || (runtime.len() >= 3 && runtime[..3] == [0xEF, 0xBB, 0xBF])
        {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 This looks like a source file, not a compiled binary.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        } else if (runtime[0] == 0x50 && runtime[1] == 0x4B)  // ZIP (PK)
            || (runtime[0] == 0x1F && runtime[1] == 0x8B)      // gzip
            || (runtime[..4] == [0xFD, 0x37, 0x7A, 0x58])      // xz
        {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 This looks like an archive. Extract it first, or omit --runtime to let sema download automatically.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        } else {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 The --runtime file doesn't appear to be a valid sema executable.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        }
    })?;

    match format {
        cross_compile::BinaryFormat::MachO => {
            let mut out = std::fs::File::create(output_path)?;
            libsui::Macho::from(runtime)?
                .write_section("semaexec", archive_bytes.to_vec())?
                .build_and_sign(&mut out)?;
        }
        cross_compile::BinaryFormat::Pe => {
            // Ordering invariant: the LAST writer must be one that serializes
            // the resource directory SORTED (named-before-ID, ascending) —
            // the PE spec requirement behind FindResource's binary search.
            // libsui's own writer emits insertion order, which the Win32 API
            // cannot see (ERROR_RESOURCE_TYPE_NOT_FOUND) even though linear
            // parsers can — every `sema build` exe booted as the bare REPL on
            // Windows (docs/bugs/archive/2026-07-29-windows-product-bugs.md bug 1).
            // So: libsui embeds the payload + icon first, and the editpe
            // (>=0.2, sorting) version-info pass runs last, re-serializing
            // the whole tree — payload, icons, and VERSIONINFO all survive
            // and are API-visible.
            let mut branded = Vec::with_capacity(runtime.len() + archive_bytes.len());
            libsui::PortableExecutable::from(&runtime)?
                .write_resource("semaexec", archive_bytes.to_vec())?
                // The rounded mark carries its own dark tile, so it reads correctly on
                // both light and dark backgrounds — PE icons cannot adapt to theme.
                // In-crate copy, not the repo-root `assets/`: `cargo package` ships
                // only files under the package root, so embedding across the crate
                // boundary compiles here and breaks the published crate. Synced from
                // canonical by `scripts/gen-icon-assets.py` (`jake icons-assets`).
                .set_icon(include_bytes!("../assets/sema-mark-rounded-512.png"))?
                .build(&mut branded)?;
            let branded = set_windows_version_info(branded, output_path)?;
            std::fs::write(output_path, branded)?;
        }
        cross_compile::BinaryFormat::Elf => {
            archive::write_bundled_executable_from_bytes(&runtime, output_path, archive_bytes)?;
            return Ok(());
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(output_path, perms)?;
    }

    Ok(())
}

/// Recursively collect files from a directory into the VFS files map.
fn collect_directory_files(
    dir: &std::path::Path,
    base: &str,
    files: &mut std::collections::HashMap<String, Vec<u8>>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            print_cli_warning(format!("cannot read directory {}: {e}", dir.display()));
            return;
        }
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let vfs_path = if base.is_empty() {
            name.clone()
        } else {
            format!("{base}/{name}")
        };

        if entry_path.is_dir() {
            collect_directory_files(&entry_path, &vfs_path, files);
        } else if entry_path.is_file() {
            if let Err(e) = sema_core::vfs::validate_vfs_path(&vfs_path) {
                print_cli_warning(format!("skipping {}: {e}", entry_path.display()));
                continue;
            }
            match std::fs::read(&entry_path) {
                Ok(data) => {
                    files.insert(vfs_path, data);
                }
                Err(e) => {
                    print_cli_warning(format!("cannot read {}: {e}", entry_path.display()));
                }
            }
        }
    }
}

/// Return current Unix timestamp as a string (seconds since epoch).
fn build_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn run_check(file: &str) {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            print_cli_error(format!("could not read {file}: {e}"));
            std::process::exit(1);
        }
    };

    if !sema_vm::is_bytecode_file(&bytes) {
        print_cli_error(format!("{file} is not a valid .semac bytecode file"));
        std::process::exit(1);
    }

    // Read header info before full deserialization
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let major = u16::from_le_bytes([bytes[8], bytes[9]]);
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    let patch = u16::from_le_bytes([bytes[12], bytes[13]]);

    match sema_vm::deserialize_from_bytes(&bytes) {
        Ok(result) => {
            let n_funcs = result.functions.len();
            println!(
                "✓ {file}: valid (format v{format_version}, sema {major}.{minor}.{patch}, {n_funcs} function{}, {} bytes)",
                if n_funcs == 1 { "" } else { "s" },
                bytes.len()
            );
        }
        Err(e) => {
            print_cli_error(format!("{file} is invalid: {}", e.format_plain()));
            std::process::exit(1);
        }
    }
}

fn run_disasm(file: &str, json: bool) {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            let msg = match e.kind() {
                std::io::ErrorKind::NotFound => format!("file not found: {file}"),
                std::io::ErrorKind::PermissionDenied => format!("permission denied: {file}"),
                _ => format!("reading {file}: {e}"),
            };
            print_cli_error(msg);
            std::process::exit(1);
        }
    };

    if !sema_vm::is_bytecode_file(&bytes) {
        print_cli_error(format!("{file} is not a valid .semac bytecode file"));
        std::process::exit(1);
    }

    let result = match sema_vm::deserialize_from_bytes(&bytes) {
        Ok(r) => r,
        Err(e) => {
            print_cli_error(format!("deserialization failed: {}", e.format_plain()));
            std::process::exit(1);
        }
    };

    if json {
        let json_val = disassemble_to_json(&result, &bytes);
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        // Disassemble main chunk
        print!("{}", sema_vm::disassemble(&result.chunk, Some("<main>")));

        // Disassemble each function
        for (i, func) in result.functions.iter().enumerate() {
            let name = func
                .name
                .map(sema_core::resolve)
                .unwrap_or_else(|| format!("<fn {i}>"));
            print!("{}", sema_vm::disassemble(&func.chunk, Some(&name)));
        }
    }
}

fn disassemble_to_json(result: &sema_vm::CompileResult, bytes: &[u8]) -> serde_json::Value {
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let major = u16::from_le_bytes([bytes[8], bytes[9]]);
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    let patch = u16::from_le_bytes([bytes[12], bytes[13]]);

    let mut functions = Vec::new();

    // Main chunk
    functions.push(chunk_to_json(&result.chunk, "<main>"));

    // Function templates
    for (i, func) in result.functions.iter().enumerate() {
        let name = func
            .name
            .map(sema_core::resolve)
            .unwrap_or_else(|| format!("<fn {i}>"));
        let mut obj = chunk_to_json(&func.chunk, &name);
        obj["arity"] = serde_json::json!(func.arity);
        obj["has_rest"] = serde_json::json!(func.has_rest);
        obj["upvalues"] = serde_json::json!(func.upvalue_descs.len());
        functions.push(obj);
    }

    serde_json::json!({
        "format_version": format_version,
        "sema_version": format!("{major}.{minor}.{patch}"),
        "size_bytes": bytes.len(),
        "functions": functions,
    })
}

fn chunk_to_json(chunk: &sema_vm::Chunk, name: &str) -> serde_json::Value {
    let mut instructions = Vec::new();
    let code = &chunk.code;
    let mut pc = 0usize;

    while pc < code.len() {
        let op_byte = code[pc];
        let op = sema_vm::Op::from_u8(op_byte);
        let op_name = op
            .map(|o| format!("{o:?}"))
            .unwrap_or_else(|| format!("Unknown(0x{op_byte:02x})"));

        let (inst, next_pc) = match op {
            Some(sema_vm::Op::Const) => {
                let idx = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                let val_str = chunk
                    .consts
                    .get(idx as usize)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into());
                (
                    serde_json::json!({"pc": pc, "op": op_name, "index": idx, "value": val_str}),
                    pc + 3,
                )
            }
            Some(
                sema_vm::Op::LoadLocal
                | sema_vm::Op::TakeLocal
                | sema_vm::Op::StoreLocal
                | sema_vm::Op::LoadUpvalue
                | sema_vm::Op::StoreUpvalue,
            ) => {
                let slot = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "slot": slot}),
                    pc + 3,
                )
            }
            Some(sema_vm::Op::StoreGlobal | sema_vm::Op::DefineGlobal) => {
                let spur_bits =
                    u32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                // The deserialized bytecode has already remapped indices to valid Spurs.
                let name_str = if spur_bits != 0 {
                    let spur = sema_core::bits_to_spur(spur_bits);
                    sema_core::resolve(spur)
                } else {
                    format!("spur({spur_bits})")
                };
                (
                    serde_json::json!({"pc": pc, "op": op_name, "name": name_str}),
                    pc + 5,
                )
            }
            Some(sema_vm::Op::LoadGlobal) => {
                let spur_bits =
                    u32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                let name_str = if spur_bits != 0 {
                    let spur = sema_core::bits_to_spur(spur_bits);
                    sema_core::resolve(spur)
                } else {
                    format!("spur({spur_bits})")
                };
                let cache_slot = u16::from_le_bytes([code[pc + 5], code[pc + 6]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "name": name_str, "cache_slot": cache_slot}),
                    pc + 7,
                )
            }
            Some(sema_vm::Op::CallGlobal) => {
                let spur_bits =
                    u32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                let name_str = if spur_bits != 0 {
                    let spur = sema_core::bits_to_spur(spur_bits);
                    sema_core::resolve(spur)
                } else {
                    format!("spur({spur_bits})")
                };
                let argc = u16::from_le_bytes([code[pc + 5], code[pc + 6]]);
                let cache_slot = u16::from_le_bytes([code[pc + 7], code[pc + 8]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "name": name_str, "argc": argc, "cache_slot": cache_slot}),
                    pc + 9,
                )
            }
            Some(sema_vm::Op::Jump | sema_vm::Op::JumpIfFalse | sema_vm::Op::JumpIfTrue) => {
                let offset =
                    i32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                let target = (pc as i32 + 5 + offset) as u32;
                (
                    serde_json::json!({"pc": pc, "op": op_name, "offset": offset, "target": target}),
                    pc + 5,
                )
            }
            Some(
                sema_vm::Op::Call
                | sema_vm::Op::TailCall
                | sema_vm::Op::SelfTailCall
                | sema_vm::Op::CallSelf,
            ) => {
                let argc = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "argc": argc}),
                    pc + 3,
                )
            }
            Some(sema_vm::Op::CallNative) => {
                let native_id = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                let argc = u16::from_le_bytes([code[pc + 3], code[pc + 4]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "native_id": native_id, "argc": argc}),
                    pc + 5,
                )
            }
            Some(sema_vm::Op::MakeClosure) => {
                let func_id = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                let n_upvalues = u16::from_le_bytes([code[pc + 3], code[pc + 4]]);
                let mut upvals = Vec::new();
                let mut upc = pc + 5;
                for _ in 0..n_upvalues {
                    let is_local = u16::from_le_bytes([code[upc], code[upc + 1]]);
                    let idx = u16::from_le_bytes([code[upc + 2], code[upc + 3]]);
                    upvals.push(serde_json::json!({"is_local": is_local != 0, "index": idx}));
                    upc += 4;
                }
                (
                    serde_json::json!({"pc": pc, "op": op_name, "func_id": func_id, "upvalues": upvals}),
                    upc,
                )
            }
            Some(
                sema_vm::Op::MakeList
                | sema_vm::Op::MakeVector
                | sema_vm::Op::MakeMap
                | sema_vm::Op::MakeHashMap,
            ) => {
                let count = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "count": count}),
                    pc + 3,
                )
            }
            _ => (serde_json::json!({"pc": pc, "op": op_name}), pc + 1),
        };

        instructions.push(inst);
        pc = next_pc;
    }

    let constants: Vec<String> = chunk.consts.iter().map(|v| v.to_string()).collect();

    serde_json::json!({
        "name": name,
        "n_locals": chunk.n_locals,
        "max_stack": chunk.max_stack,
        "code_bytes": chunk.code.len(),
        "constants": constants,
        "instructions": instructions,
        "exception_table": chunk.exception_table.iter().map(|e| {
            serde_json::json!({
                "try_start": e.try_start,
                "try_end": e.try_end,
                "handler_pc": e.handler_pc,
                "stack_depth": e.stack_depth,
                "catch_slot": e.catch_slot,
            })
        }).collect::<Vec<_>>(),
    })
}

fn run_bytecode_bytes(
    interpreter: &Interpreter,
    bytes: &[u8],
) -> Result<sema_core::Value, SemaError> {
    let result = sema_vm::deserialize_from_bytes(bytes)?;

    let functions: Vec<std::rc::Rc<sema_vm::Function>> =
        result.functions.into_iter().map(std::rc::Rc::new).collect();
    let main_cache_slots = result.chunk.n_global_cache_slots;
    let closure = std::rc::Rc::new(sema_vm::Closure {
        func: std::rc::Rc::new(sema_vm::Function {
            name: None,
            chunk: result.chunk,
            upvalue_descs: Vec::new(),
            upvalue_names: Vec::new(),
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: Vec::new(),
            local_scopes: Vec::new(),
            source_file: None,
            cache_offset: 0,
            suspend_cache: std::cell::Cell::new(None),
        }),
        upvalues: Vec::new(),
        // Top-level main closure: uses the VM's own globals and function table.
        globals: None,
        functions: None,
    });

    let mut vm = sema_vm::VM::new(
        interpreter.global_env.clone(),
        functions,
        &[],
        main_cache_slots,
    )?;
    // Drive the `.semac` program on the interpreter's unified cooperative
    // runtime, the sole async engine, so async/await, channels, and timers work
    // in compiled bytecode (top-level or inside a `(load ...)`). A `.semac`
    // carries no native table (the format is process-local), and bytecode
    // compiled with `known_natives=None` uses CallGlobal rather than CallNative,
    // so task VMs resolve natives via the shared global env — the empty native
    // table passed to `VM::new` is correct here.
    vm.seed_main_frame(closure);
    interpreter.drive_vm_on_runtime(vm)
}

fn run_fmt(
    patterns: &[String],
    check: bool,
    show_diff: bool,
    opts: &sema_fmt::FormatOptions,
    ignore: &[String],
    json: bool,
) {
    // A path is ignored when it matches an `ignore` entry from sema.toml.
    // An entry with glob characters matches as a glob; anything else is a
    // literal path prefix (file or directory). Paths are matched relative to
    // the working directory, `./`-stripped.
    let is_ignored = |path: &str| -> bool {
        // Walked paths carry the host separator (`\` on Windows) while ignore
        // entries are written with `/`; compare in `/` form or literal-prefix
        // entries never match there (globs matched either way).
        let unified = path.replace('\\', "/");
        let normalized = unified.strip_prefix("./").unwrap_or(&unified);
        ignore.iter().any(|pat| {
            if pat.contains('*') || pat.contains('?') || pat.contains('[') {
                glob::Pattern::new(pat)
                    .map(|g| g.matches(normalized))
                    .unwrap_or(false)
            } else {
                let prefix = pat.trim_end_matches('/');
                normalized == prefix || normalized.starts_with(&format!("{prefix}/"))
            }
        })
    };
    // Wildcards don't cross a leading dot: the recursive walk stays out of
    // hidden directories (.git, .worktrees, ...) unless a pattern names one
    // literally.
    let match_opts = glob::MatchOptions {
        require_literal_leading_dot: true,
        ..Default::default()
    };
    // Handle stdin ("-")
    if patterns.len() == 1 && patterns[0] == "-" {
        let mut source = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut source) {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "formatted": false,
                        "error": format!("Error reading stdin: {e}")
                    })
                );
            } else {
                print_cli_error(format!("could not read stdin: {e}"));
            }
            std::process::exit(1);
        }
        match sema_fmt::format_source(&source, opts) {
            Ok(formatted) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "formatted": true,
                            "source": formatted
                        })
                    );
                } else {
                    print!("{formatted}");
                }
            }
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "formatted": false,
                            "error": format!("{e}")
                        })
                    );
                } else {
                    print_cli_error(format!("could not format stdin: {e}"));
                }
                std::process::exit(1);
            }
        }
        return;
    }

    // Determine which files to format
    let files = if patterns.is_empty() {
        // Default: all .sema files in current directory recursively
        match glob::glob_with("**/*.sema", match_opts) {
            Ok(paths) => paths
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .filter(|p| !is_ignored(p))
                .collect::<Vec<_>>(),
            Err(e) => {
                print_cli_error(format!("invalid glob pattern: {e}"));
                std::process::exit(1);
            }
        }
    } else {
        // Expand each pattern
        let mut all_files = Vec::new();
        for pattern in patterns {
            // If it contains glob characters, expand it
            if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
                match glob::glob_with(pattern, match_opts) {
                    Ok(paths) => {
                        for path in paths.filter_map(|p| p.ok()) {
                            let path = path.to_string_lossy().to_string();
                            if !is_ignored(&path) {
                                all_files.push(path);
                            }
                        }
                    }
                    Err(e) => {
                        print_cli_error(format!("invalid glob pattern '{pattern}': {e}"));
                        std::process::exit(1);
                    }
                }
            } else if std::path::Path::new(pattern).is_dir() {
                // A directory means every .sema file under it (`sema fmt .`).
                let dir_glob = format!("{}/**/*.sema", pattern.trim_end_matches(['/', '\\']));
                match glob::glob_with(&dir_glob, match_opts) {
                    Ok(paths) => {
                        for path in paths.filter_map(|p| p.ok()) {
                            let path = path.to_string_lossy().to_string();
                            if !is_ignored(&path) {
                                all_files.push(path);
                            }
                        }
                    }
                    Err(e) => {
                        print_cli_error(format!("invalid glob pattern '{dir_glob}': {e}"));
                        std::process::exit(1);
                    }
                }
            } else {
                // An explicitly named file always formats, ignore list or not
                all_files.push(pattern.clone());
            }
        }
        all_files
    };

    if files.is_empty() {
        if !json {
            println!("No .sema files found");
        }
        return;
    }

    let mut checked = 0;
    let mut changed = 0;
    let mut errors = 0;

    for file in &files {
        let source = match read_source_file(file) {
            Ok(s) => s,
            Err(msg) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "file": file,
                            "formatted": false,
                            "error": msg,
                        })
                    );
                } else {
                    print_cli_error(msg);
                }
                errors += 1;
                continue;
            }
        };

        let formatted = match sema_fmt::format_source(&source, opts) {
            Ok(f) => f,
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "file": file,
                            "formatted": false,
                            "error": format!("Error formatting {file}: {e}")
                        })
                    );
                } else {
                    print_cli_error(format!("could not format {file}: {e}"));
                }
                errors += 1;
                continue;
            }
        };

        checked += 1;
        let file_changed = source != formatted;
        if file_changed {
            changed += 1;
        }

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "file": file,
                    "formatted": true,
                    "changed": file_changed,
                    "source": formatted
                })
            );
            continue;
        }

        if file_changed {
            if check {
                println!("Would reformat: {file}");
            } else if show_diff {
                // Simple line-by-line diff
                print_simple_diff(file, &source, &formatted);
            } else {
                // Write formatted output back
                if let Err(e) = std::fs::write(file, &formatted) {
                    print_cli_error(format!("could not write {file}: {e}"));
                    errors += 1;
                    continue;
                }
                println!("Formatted: {file}");
            }
        }
    }

    // Print summary
    if !json {
        if check {
            if changed > 0 {
                println!("\n{changed} file(s) would be reformatted, {checked} file(s) checked");
                std::process::exit(1);
            } else {
                println!("{checked} file(s) already formatted");
            }
        } else if show_diff {
            println!("\n{changed} file(s) would change, {checked} file(s) checked");
        } else if changed > 0 {
            println!(
                "\n{changed} file(s) formatted, {} file(s) unchanged",
                checked - changed
            );
        } else {
            println!("{checked} file(s) already formatted");
        }
    }

    if errors > 0 {
        print_cli_error(format!("{errors} file(s) could not be formatted"));
        std::process::exit(1);
    }

    if check && changed > 0 {
        std::process::exit(1);
    }
}

fn print_simple_diff(filename: &str, old: &str, new: &str) {
    println!("--- {filename}");
    println!("+++ {filename}");
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Simple context diff: show lines that differ
    let max_lines = old_lines.len().max(new_lines.len());
    let mut in_diff = false;
    let mut diff_start = 0;

    for i in 0..max_lines {
        let old_line = old_lines.get(i).copied().unwrap_or("");
        let new_line = new_lines.get(i).copied().unwrap_or("");

        if old_line != new_line {
            if !in_diff {
                diff_start = i;
                in_diff = true;
                println!("@@ -{} +{} @@", i + 1, i + 1);
            }
            if i < old_lines.len() {
                println!("-{old_line}");
            }
            if i < new_lines.len() {
                println!("+{new_line}");
            }
        } else if in_diff && i - diff_start < 3 {
            println!(" {old_line}");
        } else {
            in_diff = false;
        }
    }
}

fn run_ast(file: Option<String>, eval: Option<String>, json: bool) {
    let source = match (&file, &eval) {
        (Some(path), None) => match read_source_file(path) {
            Ok(content) => content,
            Err(msg) => {
                print_cli_error(msg);
                std::process::exit(1);
            }
        },
        (None, Some(expr)) => expr.clone(),
        (Some(_), Some(_)) => {
            print_cli_error("cannot specify both a file and --eval");
            std::process::exit(1);
        }
        (None, None) => {
            print_cli_error("provide a file or --eval expression");
            std::process::exit(1);
        }
    };

    let exprs = match sema_reader::read_many(&source) {
        Ok(exprs) => exprs,
        Err(e) => {
            print_cli_error(format!("parsing failed: {}", e.format_plain()));
            std::process::exit(1);
        }
    };

    if json {
        let json_ast: Vec<serde_json::Value> = exprs.iter().map(value_to_ast_json).collect();
        let output = if json_ast.len() == 1 {
            serde_json::to_string_pretty(&json_ast[0]).unwrap()
        } else {
            serde_json::to_string_pretty(&json_ast).unwrap()
        };
        println!("{output}");
    } else {
        for (i, expr) in exprs.iter().enumerate() {
            if i > 0 {
                println!();
            }
            print_ast(expr, 0);
        }
    }
}

fn value_to_ast_json(val: &Value) -> serde_json::Value {
    match val.view() {
        ValueView::Nil => serde_json::Value::Object(
            [("type".to_string(), serde_json::Value::String("nil".into()))]
                .into_iter()
                .collect(),
        ),
        ValueView::Bool(b) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("bool".into())),
                ("value".to_string(), serde_json::Value::Bool(b)),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Int(n) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("int".into())),
                ("value".to_string(), serde_json::Value::Number(n.into())),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Float(f) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("float".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Number::from_f64(f)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::String(s) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("string".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Value::String(s.to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Symbol(s) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("symbol".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Value::String(sema_core::resolve(s)),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Keyword(s) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("keyword".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Value::String(sema_core::resolve(s)),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::List(items) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("list".into())),
                (
                    "children".to_string(),
                    serde_json::Value::Array(items.iter().map(value_to_ast_json).collect()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Vector(items) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("vector".into()),
                ),
                (
                    "children".to_string(),
                    serde_json::Value::Array(items.iter().map(value_to_ast_json).collect()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Map(map) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("map".into())),
                (
                    "entries".to_string(),
                    serde_json::Value::Array(
                        map.iter()
                            .map(|(k, v)| {
                                serde_json::Value::Object(
                                    [
                                        ("key".to_string(), value_to_ast_json(k)),
                                        ("value".to_string(), value_to_ast_json(v)),
                                    ]
                                    .into_iter()
                                    .collect(),
                                )
                            })
                            .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        _ => serde_json::Value::Object(
            [(
                "type".to_string(),
                serde_json::Value::String(val.type_name().into()),
            )]
            .into_iter()
            .collect(),
        ),
    }
}

fn print_ast(val: &Value, indent: usize) {
    let pad = "  ".repeat(indent);
    match val.view() {
        ValueView::Nil => println!("{pad}Nil"),
        ValueView::Bool(b) => println!("{pad}Bool {b}"),
        ValueView::Int(n) => println!("{pad}Int {n}"),
        ValueView::Float(f) => println!("{pad}Float {f}"),
        ValueView::String(s) => println!("{pad}String {s:?}"),
        ValueView::Symbol(s) => println!("{pad}Symbol {}", sema_core::resolve(s)),
        ValueView::Keyword(s) => println!("{pad}Keyword :{}", sema_core::resolve(s)),
        ValueView::List(items) => {
            println!("{pad}List");
            for item in items.iter() {
                print_ast(item, indent + 1);
            }
        }
        ValueView::Vector(items) => {
            println!("{pad}Vector");
            for item in items.iter() {
                print_ast(item, indent + 1);
            }
        }
        ValueView::Map(map) => {
            println!("{pad}Map");
            for (k, v) in map.iter() {
                println!("{pad}  Entry");
                print_ast(k, indent + 2);
                print_ast(v, indent + 2);
            }
        }
        _ => println!("{pad}{}", val.type_name()),
    }
}

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

fn generate_completions(shell: Shell) -> String {
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "sema", &mut buf);
    let mut out = String::from_utf8(buf).expect("clap completion output is utf-8");
    if shell == Shell::Zsh {
        out = fix_zsh_root_completion(out);
    }
    out.push_str(dynamic_doc_completion_script(shell));
    out
}

/// Repair subcommand completion in the generated zsh script.
///
/// `clap_complete`'s zsh generator emits the top-level optional positionals
/// (`FILE`, `SCRIPT_ARGS`) *before* the subcommand slot — even with
/// `args_conflicts_with_subcommands` set — so zsh consumes `sema notebook` as
/// the FILE positional: `sema <TAB>` offers only files and
/// `sema notebook <TAB>` completes script arguments. Subcommand completion
/// never engages, at any depth.
///
/// The repair makes position 1 an alternation of subcommands and script files
/// (`_sema_root`), and re-indexes the subcommand dispatch from `$line[3]` to
/// `$line[1]`. Every rewrite is anchored on the exact generator output; if an
/// anchor is missing (a future clap_complete changed shape), the script is
/// returned UNMODIFIED — a wrong-but-consistent script beats a broken one —
/// and the pinning unit test fails loudly so the anchors get refreshed.
///
/// zsh is the ONLY affected shell: its generator dispatches by positional
/// index (`$line[N]`), while bash (word-walk), fish
/// (`__fish_seen_subcommand_from`), elvish and powershell (name-keyed maps)
/// all match literal subcommand names — verified empirically 2026-07-03
/// (bash 5.2 in a clean container; fish `complete -C`; pwsh
/// `CommandCompletion::CompleteInput`; elvish statically).
fn fix_zsh_root_completion(script: String) -> String {
    const POSITIONALS: &str = "'::file -- File to execute:_default' \\\n\
'::script_args -- Arguments passed to the script (after --):_default' \\\n\
\":: :_sema_commands\" \\\n";
    const ROOT_SLOT: &str = "\":: :_sema_root\" \\\n";
    let anchors_present = script.contains(POSITIONALS)
        && script.contains("words=($line[3] \"${words[@]}\")")
        && script.contains("case $line[3] in");
    if !anchors_present {
        return script;
    }
    let mut out = script.replacen(POSITIONALS, ROOT_SLOT, 1);
    out = out.replacen(
        "words=($line[3] \"${words[@]}\")",
        "words=($line[1] \"${words[@]}\")",
        1,
    );
    out = out.replacen(
        "curcontext=\"${curcontext%:*:*}:sema-command-$line[3]:\"",
        "curcontext=\"${curcontext%:*:*}:sema-command-$line[1]:\"",
        1,
    );
    out = out.replacen("case $line[3] in", "case $line[1] in", 1);
    // The definition must precede clap's self-invoking trailer
    // (`if [ "$funcstack[1]" = "_sema" ]; then _sema "$@" ...`): on the very
    // first TAB the file executes top-to-bottom and calls `_sema` right there —
    // a root fn appended after the trailer is not yet defined at that moment.
    let root_fn = "\n_sema_root() {\n    _alternative \\\n        'subcommands:sema command:_sema_commands' \\\n        'files:script file:_files'\n}\n\n";
    const TRAILER: &str = "if [ \"$funcstack[1]\" = \"_sema\" ]; then";
    if let Some(pos) = out.find(TRAILER) {
        out.insert_str(pos, root_fn);
    } else {
        out.push_str(root_fn);
    }
    out
}

fn dynamic_doc_completion_script(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => {
            r#"

# Dynamic Sema doc symbol completion.
_sema_doc_complete() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    if [[ ${COMP_WORDS[1]} == doc && ${COMP_CWORD} -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "$(sema __complete-doc-symbols "$cur")" -- "$cur") )
        return
    fi
    if [[ ${COMP_WORDS[1]} == doc && ${COMP_WORDS[2]} == show && ${COMP_CWORD} -eq 3 ]]; then
        COMPREPLY=( $(compgen -W "$(sema __complete-doc-symbols "$cur")" -- "$cur") )
        return
    fi
    _sema "$@"
}
complete -o nosort -o bashdefault -o default -F _sema_doc_complete sema
"#
        }
        Shell::Zsh => {
            r#"

# Dynamic Sema doc symbol completion.
_sema_doc_complete() {
  if (( CURRENT == 3 )) && [[ "${words[2]}" == "doc" ]]; then
    local -a matches
    matches=("${(@f)$(sema __complete-doc-symbols "${words[CURRENT]}")}")
    _describe 'Sema doc symbol' matches
    return
  fi
  if (( CURRENT == 4 )) && [[ "${words[2]}" == "doc" && "${words[3]}" == "show" ]]; then
    local -a matches
    matches=("${(@f)$(sema __complete-doc-symbols "${words[CURRENT]}")}")
    _describe 'Sema doc symbol' matches
    return
  fi
  _sema "$@"
}
compdef _sema_doc_complete sema
"#
        }
        Shell::Fish => {
            r#"

# Dynamic Sema doc symbol completion.
complete -c sema -n '__fish_seen_subcommand_from doc; and not __fish_seen_subcommand_from show search apropos' -a '(sema __complete-doc-symbols (commandline -ct))'
complete -c sema -n '__fish_seen_subcommand_from doc show' -a '(sema __complete-doc-symbols (commandline -ct))'
"#
        }
        _ => "",
    }
}

fn install_completions(shell: Shell) {
    let home = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            print_cli_error("could not determine the home directory");
            std::process::exit(1);
        }
    };

    let path = match shell {
        Shell::Zsh => home.join(".zsh/completions/_sema"),
        Shell::Bash => home.join(".local/share/bash-completion/completions/sema"),
        Shell::Fish => home.join(".config/fish/completions/sema.fish"),
        Shell::Elvish => home.join(".config/elvish/lib/sema.elv"),
        Shell::PowerShell => {
            print_cli_error(
                "Auto-install is not supported for PowerShell.\n\
                 Run manually: sema completions powershell >> $PROFILE",
            );
            std::process::exit(1);
        }
        _ => {
            print_cli_error("auto-install is not supported for this shell");
            std::process::exit(1);
        }
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            print_cli_error(format!(
                "could not create directory {}: {e}",
                parent.display()
            ));
            std::process::exit(1);
        });
    }

    let completions = generate_completions(shell);
    std::fs::write(&path, completions).unwrap_or_else(|e| {
        print_cli_error(format!("could not write {}: {e}", path.display()));
        std::process::exit(1);
    });

    println!("✓ Installed {shell} completions to {}", path.display());
    if shell == Shell::Zsh {
        println!("  Add to ~/.zshrc (before compinit): fpath=(~/.zsh/completions $fpath)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn approval_revision_embeds_absolute_import_identity() {
        let root = std::env::temp_dir().join(format!(
            "sema-approval-revision-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let helper = root.join("helper.sema");
        let workflow = root.join("workflow.sema");
        let helper_bytes = b"(define target \"v1\")\n";
        std::fs::write(&helper, helper_bytes).unwrap();
        let source = format!(
            "(import \"helper.sema\")\n(import {})",
            serde_json::to_string(&helper).unwrap()
        );
        std::fs::write(&workflow, &source).unwrap();

        let revision = workflow_approval_revision(&workflow, source.as_bytes()).unwrap();
        assert!(revision
            .embedded_dependencies
            .iter()
            .any(|(path, bytes)| path == &helper && bytes == helper_bytes));

        let _ = std::fs::remove_dir_all(root);
    }

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

    #[test]
    fn build_summary_helpers() {
        assert_eq!(
            triple_os_arch("aarch64-apple-darwin"),
            ("macos", "arm64".into())
        );
        assert_eq!(
            triple_os_arch("x86_64-unknown-linux-gnu"),
            ("linux", "x86_64".into())
        );
        assert_eq!(
            triple_os_arch("x86_64-pc-windows-msvc"),
            ("windows", "x86_64".into())
        );
        assert_eq!(triple_os_arch("web"), ("web", "-".into()));

        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(12_700), "12.4 KB");
        assert_eq!(human_size(29_289_346), "27.9 MB");
        assert_eq!(human_size(2_147_483_648), "2.0 GB");

        assert_eq!(
            human_duration(std::time::Duration::from_millis(640)),
            "0.6s"
        );
        assert_eq!(human_duration(std::time::Duration::from_secs(92)), "1m 32s");
    }

    #[test]
    fn build_summary_table_aligns_and_reports_failures() {
        let artifacts = vec![
            BuiltArtifact {
                target: "aarch64-apple-darwin".into(),
                path: "/x/game-aarch64-apple-darwin".into(),
                bytes: 29_289_346,
                runtime: Some("host"),
                duration: std::time::Duration::from_secs(1),
            },
            BuiltArtifact {
                target: "x86_64-unknown-linux-gnu".into(),
                path: "/x/game-x86_64-unknown-linux-gnu".into(),
                bytes: 32_462_497,
                runtime: Some("downloaded"),
                duration: std::time::Duration::from_secs(3),
            },
        ];
        let failures = vec![(
            "x86_64-pc-windows-msvc".to_string(),
            "download failed: 404".to_string(),
        )];
        let table = render_build_summary(&artifacts, &failures, std::time::Duration::from_secs(14));
        assert!(table.starts_with("Built 2/3 targets in 14.0s:"), "{table}");
        // every data row aligns: the path column starts at the same byte offset
        let rows: Vec<&str> = table.lines().skip(2).collect();
        assert_eq!(rows.len(), 3);
        let col = rows[0].find("/x/").unwrap();
        assert_eq!(rows[1].find("/x/"), Some(col), "{table}");
        assert!(rows[2].contains("FAILED"));
        assert!(rows[2].contains("download failed: 404"));
        assert!(!table.contains("won't run"));
    }

    #[test]
    fn build_output_default_name_adds_exe_for_windows_targets() {
        let src = std::path::Path::new("examples/game-of-life.sema");
        // No --target builds for the host, so the default name carries the
        // HOST's exe suffix (".exe" on a Windows host, "" elsewhere).
        assert_eq!(
            default_output_name(src, None),
            format!("game-of-life{}", std::env::consts::EXE_SUFFIX)
        );
        assert_eq!(
            default_output_name(src, Some("windows")),
            "game-of-life.exe"
        );
        assert_eq!(
            default_output_name(src, Some("x86_64-pc-windows-msvc")),
            "game-of-life.exe"
        );
        assert_eq!(default_output_name(src, Some("linux")), "game-of-life");
    }

    #[test]
    fn build_output_into_existing_dir_uses_default_filename() {
        let dir = std::env::temp_dir().join(format!("sema-out-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = std::path::Path::new("hello.sema");
        let out = resolve_output_path(Some(dir.to_str().unwrap()), src, None);
        assert_eq!(
            out,
            dir.join(format!("hello{}", std::env::consts::EXE_SUFFIX))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_output_trailing_slash_means_directory_even_if_missing() {
        let src = std::path::Path::new("hello.sema");
        let out = resolve_output_path(Some("no/such/dir/"), src, None);
        assert_eq!(
            out,
            std::path::PathBuf::from(format!("no/such/dir/hello{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn build_output_plain_path_is_the_file_itself() {
        let src = std::path::Path::new("hello.sema");
        let out = resolve_output_path(Some("dist/game"), src, None);
        assert_eq!(out, std::path::Path::new("dist/game"));
    }

    #[test]
    fn build_output_expands_leading_tilde() {
        // `--output=~/x` reaches us unexpanded (the ~ follows `=`); it must never
        // be treated as a literal `./~` directory.
        if let Ok(home) = std::env::var("HOME") {
            let src = std::path::Path::new("hello.sema");
            let out = resolve_output_path(Some("~/sema-out/game"), src, None);
            assert_eq!(out, std::path::Path::new(&home).join("sema-out/game"));
        }
    }

    #[test]
    fn build_all_targets_suffixes_filenames_and_honors_output_file_base() {
        let src = std::path::Path::new("examples/game-of-life.sema");
        // file-ish output: base name is taken from the output path
        let plans = plan_all_target_outputs(Some("dist/game"), src);
        assert_eq!(plans.len(), cross_compile::SUPPORTED_TARGETS.len());
        for (t, path) in &plans {
            let expect_ext = if cross_compile::is_windows_target(t) {
                ".exe"
            } else {
                ""
            };
            assert_eq!(
                path,
                &std::path::PathBuf::from(format!("dist/game-{t}{expect_ext}")),
                "unexpected plan for {t}"
            );
        }
        // every planned path is distinct — this is the bug that motivated the plan
        let unique: std::collections::HashSet<_> = plans.iter().map(|(_, p)| p).collect();
        assert_eq!(unique.len(), plans.len());
    }

    #[test]
    fn build_all_targets_strips_exe_from_output_base() {
        let src = std::path::Path::new("hello.sema");
        let plans = plan_all_target_outputs(Some("dist/game.exe"), src);
        assert!(plans
            .iter()
            .all(|(_, p)| !p.to_string_lossy().contains(".exe-")));
        assert!(plans
            .iter()
            .any(|(t, p)| cross_compile::is_windows_target(t)
                && p.to_string_lossy().ends_with(".exe")));
    }

    #[test]
    fn build_all_targets_into_directory_uses_source_stem() {
        let dir = std::env::temp_dir().join(format!("sema-all-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = std::path::Path::new("examples/game-of-life.sema");
        let plans = plan_all_target_outputs(Some(dir.to_str().unwrap()), src);
        for (t, path) in &plans {
            assert!(path.starts_with(&dir));
            assert!(path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("game-of-life-{t}")));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_all_targets_without_output_uses_cwd_stem() {
        let src = std::path::Path::new("examples/game-of-life.sema");
        let plans = plan_all_target_outputs(None, src);
        assert_eq!(
            plans[0].1,
            std::path::PathBuf::from(format!("game-of-life-{}", plans[0].0))
        );
    }

    /// Pins the zsh-completion repair (`fix_zsh_root_completion`): position 1
    /// must dispatch subcommands (clap_complete emits the FILE/SCRIPT_ARGS
    /// positionals first, which swallows the subcommand word — `sema notebook
    /// <TAB>` completed files). If this fails after a clap_complete upgrade,
    /// refresh the anchors in `fix_zsh_root_completion`.
    #[test]
    fn zsh_completions_dispatch_subcommands_at_position_one() {
        let script = generate_completions(clap_complete::Shell::Zsh);
        assert!(
            script.contains(":: :_sema_root"),
            "root slot missing — anchor drift in fix_zsh_root_completion"
        );
        assert!(
            script.contains("_sema_root() {"),
            "root alternation fn missing"
        );
        assert!(
            script.contains("case $line[1] in") && !script.contains("case $line[3] in"),
            "top-level dispatch must read the subcommand from position 1"
        );
        assert!(
            !script.contains("File to execute"),
            "top-level FILE positional must not shadow the subcommand slot"
        );
        // The nested groups must still be intact (spot-check one). clap_complete
        // names nested subcommand groups `_sema__subcmd__<name>_commands`.
        assert!(script.contains("_sema__subcmd__notebook_commands"));
    }

    use super::{compile_source_to_bytecode, run_bytecode_bytes};
    use sema_core::{intern, NativeFn, Sandbox, Value};
    use sema_eval::Interpreter;

    #[test]
    fn web_build_prelude_expands_defcomponent_into_callable_global() {
        let source = r##"
            (defcomponent counter-view ()
              [:div "ok"])
            (mount! "#app" counter-view)
        "##;

        let bytes = compile_source_to_bytecode(source).expect("compile should succeed");

        let interp = Interpreter::new_with_sandbox(&Sandbox::allow_all());
        interp.global_env.set(
            intern("component/mount!"),
            Value::native_fn(NativeFn::simple("component/mount!", |_args| {
                Ok(Value::nil())
            })),
        );

        run_bytecode_bytes(&interp, &bytes).expect("compiled program should execute");

        let counter_view = interp
            .global_env
            .get(intern("counter-view"))
            .expect("defcomponent should define counter-view");
        let rendered = sema_eval::call_value(&interp.ctx, &counter_view, &[])
            .expect("counter-view should be callable");

        assert!(!rendered.is_nil(), "component should return SIP markup");
    }

    #[test]
    fn web_build_prelude_expands_reactive_macros() {
        let source = r#"
            (def doubled (computed 42))
            (def batched (batch 1 2 3))
        "#;

        let bytes = compile_source_to_bytecode(source).expect("compile should succeed");

        let interp = Interpreter::new_with_sandbox(&Sandbox::allow_all());
        interp.global_env.set(
            intern("__state/computed-create"),
            Value::native_fn(NativeFn::simple("__state/computed-create", |_args| {
                Ok(Value::string("computed-ok"))
            })),
        );
        interp.global_env.set(
            intern("__state/batch-run"),
            Value::native_fn(NativeFn::simple("__state/batch-run", |_args| {
                Ok(Value::string("batch-ok"))
            })),
        );

        run_bytecode_bytes(&interp, &bytes).expect("compiled program should execute");

        let doubled = interp
            .global_env
            .get(intern("doubled"))
            .expect("computed should define doubled");
        let batched = interp
            .global_env
            .get(intern("batched"))
            .expect("batch should define batched");

        assert_eq!(doubled, Value::string("computed-ok"));
        assert_eq!(batched, Value::string("batch-ok"));
    }

    // ── format_needs_auth_guidance ─────────────────────────────────────────

    #[test]
    fn needs_auth_guidance_matches_the_brief_verbatim() {
        let envelope = sema_reader::read(
            r#"{:status :needs-auth
                :servers ["asana" "linear"]
                :auth [{:server "asana" :url "https://mcp.asana.com/mcp" :persist "workflow"}
                       {:server "linear" :url "https://mcp.linear.app/mcp" :persist "workflow"}]}"#,
        )
        .expect("valid sema literal");

        let expected = concat!(
            "run needs authentication for 2 MCP server(s):\n",
            "  asana   sema mcp login https://mcp.asana.com/mcp\n",
            "  linear  sema mcp login https://mcp.linear.app/mcp\n",
            "then re-run this workflow. (or authenticate from `sema workflow view`)\n",
        );
        assert_eq!(format_needs_auth_guidance(&envelope), expected);
    }

    #[test]
    fn needs_auth_guidance_single_server() {
        let envelope = sema_reader::read(
            r#"{:status :needs-auth
                :servers ["gated"]
                :auth [{:server "gated" :url "http://127.0.0.1:1/mcp" :persist "run"}]}"#,
        )
        .expect("valid sema literal");

        let out = format_needs_auth_guidance(&envelope);
        assert!(out.starts_with("run needs authentication for 1 MCP server(s):\n"));
        assert!(out.contains("  gated  sema mcp login http://127.0.0.1:1/mcp\n"));
        assert!(out
            .ends_with("then re-run this workflow. (or authenticate from `sema workflow view`)\n"));
    }

    #[test]
    fn needs_auth_guidance_missing_auth_vector_degrades_to_zero_servers() {
        let envelope = sema_reader::read(r#"{:status :needs-auth}"#).expect("valid sema literal");
        let out = format_needs_auth_guidance(&envelope);
        assert!(out.starts_with("run needs authentication for 0 MCP server(s):\n"));
    }

    // ── should_enable_interactive_auth ─────────────────────────────────────

    #[test]
    fn interactive_auth_enabled_only_when_both_streams_are_ttys() {
        assert!(should_enable_interactive_auth(true, true, None, false));
        assert!(!should_enable_interactive_auth(false, true, None, false));
        assert!(!should_enable_interactive_auth(true, false, None, false));
        assert!(!should_enable_interactive_auth(false, false, None, false));
    }

    #[test]
    fn interactive_auth_disabled_by_no_auth_prompt_even_on_a_tty() {
        assert!(!should_enable_interactive_auth(true, true, None, true));
    }

    #[test]
    fn interactive_auth_disabled_by_nonempty_ci_even_on_a_tty() {
        assert!(!should_enable_interactive_auth(
            true,
            true,
            Some("true"),
            false
        ));
        assert!(!should_enable_interactive_auth(
            true,
            true,
            Some("1"),
            false
        ));
    }

    #[test]
    fn interactive_auth_ignores_an_empty_ci_value() {
        // `CI=` (set but empty) is treated the same as unset — matches the
        // brief's "env CI is unset/empty" wording exactly.
        assert!(should_enable_interactive_auth(true, true, Some(""), false));
    }
}
