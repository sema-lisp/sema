# Native Sema test runner

**Status:** Revised implementation plan for the native Sema test runner.
Implementation has not started. This revision records the selected design after review of
the existing `sema-coder` runner, the `sema-test` registry package, current package test
suites, and the current interpreter runtime. Revised 2026-08-11.

## Selected design

| Decision | Selected design |
|---|---|
| Distribution | Ship the runner and core DSL with `sema`. Do not require the registry package named `sema-test`. |
| DSL availability | Install the DSL automatically in test-worker interpreters. Do not add the test-only names to every normal interpreter. |
| File isolation | Run each test file in a child process started from `std::env::current_exe()`. |
| Test isolation | Run registered tests in registration order in one interpreter per file. Tests in one file share state. |
| Result transport | Use a versioned internal worker protocol and one canonical Rust result model. Do not parse reporter text. |
| Existing suites | Support the current `tests.sema` and `*_test.sema` script suites during migration. |
| MCP completion | A completed run with failed tests is a valid tool result with `success: false`. It is not an MCP execution error. |
| Extensibility | Keep lifecycle and recording in the builtin facility. Permit packages to build additional matchers from the public test DSL. |

## Evidence from current Sema projects

The local projects already have two test systems:

- `../sema-coder/test.sema` discovers `tests/*_test.sema`, starts one child `sema`
  process per file, treats the child exit status as authoritative, and prints child output
  only for failed files. Its 29 test files contain approximately 513 `check` calls.
- `../pkg-packages/sema-test/package.sema` provides fail-fast assertion functions and
  `test/run`/`test/run!`. Twenty-six package suites import it and contain approximately
  1,049 assertion calls. Eleven suites build ordered offline and live test lists before
  calling `test/run!`.

These systems establish useful behavior and expose missing contracts:

- process isolation contains `exit`, crashes, and process-global state;
- explicit order is used by environment-sensitive tests;
- quiet success output and complete failure output are useful;
- text-summary parsing is fragile;
- conditional live tests need real skip and tag data;
- a globally installed test package is not a declared, locked test dependency;
- manual function lists provide no source-form capture or source locations;
- fail-fast assertions hide later independent failures in the same test;
- a macro example that catches and prints failures can still exit zero, so the runner must
  own the final status.

## Problem

Sema has `assert` and `assert=`, but it has no native test lifecycle. Users must assemble
discovery, isolation, registration, aggregation, output capture, exit status, and machine
output themselves. The existing `sema-test` package implements useful matchers, but a Sema
package cannot safely provide the host process boundary, CLI command, MCP tool, timeouts,
or crash handling.

The original plan used a fresh in-process `Interpreter` as the file boundary. That is not
sufficient. The builtin `exit` calls `std::process::exit`, so one test can terminate the
complete runner. A test can also panic in native code, hang, or mutate process-global
state. The native runner must put each file in a separate process.

## Goals

1. Add an ergonomic, automatically available test DSL under `sema test`.
2. Add deterministic discovery for current and new test-file conventions.
3. Isolate each file in a child process and preserve shared state within that file.
4. Record all independent assertion failures in one test.
5. Provide an explicit fatal assertion for dependent checks.
6. Represent pass, failure, error, skip, timeout, unexpected exit, and crash separately.
7. Capture bounded stdout and stderr without corrupting machine output.
8. Use one typed result model for CLI reporters and MCP structured content.
9. Preserve the caller's sandbox and allowed-path policy in MCP workers.
10. Support current script suites while they migrate to registered tests.
11. Make deterministic cassette replay the default for tests.
12. Keep installed binaries independent of the repository and development tools.

## Non-goals for this plan

- Replacing the Rust workspace tests.
- One process per test case.
- Parallel file execution.
- Watch mode.
- Coverage instrumentation.
- Property-based or generative testing.
- An HTML reporter.
- A general plugin ABI for custom Rust reporters.
- Automatic execution of live network tests.
- Making the test DSL available in every normal interpreter or REPL session.
- Treating arbitrary `.sema` programs as tests without an explicit path or legacy
  convention.

## Terminology

| Term | Definition |
|---|---|
| runner | The parent host component that discovers files, starts workers, aggregates results, and selects a reporter. |
| worker | A hidden child-process mode that loads and runs one test file. |
| registered test | A test declared with `deftest` and stored as a named thunk in the worker registry. |
| script test | A legacy `tests.sema` or `*_test.sema` file whose process result is treated as one test because it registered no tests. |
| assertion context | A nested label created by `test/context` while a test body runs. It is attached to assertions recorded in that body. |
| assertion | One recorded check within a registered test. |
| test failure | A completed assertion whose condition did not hold. |
| test error | An uncaught Sema error outside an assertion comparison, including an error while evaluating an assertion operand. |
| runner error | Invalid arguments, denied access, discovery failure, worker protocol failure, or another failure that prevents a valid run result. |
| full test name | The relative file path and registered test name used for display and filtering. |
| failure breadcrumb | The full test name followed by any assertion-context labels. |
| captured output | Ordered Sema stdout and stderr chunks attributed to file loading or one test. |
| worker diagnostics | Bounded raw worker stdout and stderr that bypass runtime output capture, including panic, native, and subprocess output. |

## User-facing names

New Sema operations use the `test/` namespace. `deftest` is the one unqualified name
because it is a definition macro in the existing `defun`, `defmacro`, `deftool`, and
`defworkflow` family.

```scheme
(deftest "addition works"
  (test/is (> 5 3))
  (test/is= 4 (+ 2 2))
  (test/throws (/ 1 0))
  (test/context "collections"
    (test/contains [1 2 3] 2)))
```

Do not add the original plan's unqualified `is`, `testing`, `satisfies?`, or
`has-length?` names. Names ending in `?` remain predicates that return booleans. Test
matchers record results and therefore do not use predicate names.

### `deftest`

```scheme
(deftest name [options] body ...)
```

- `name` is a non-empty string.
- `options` is an optional literal map.
- `:tags` is an optional vector of keywords.
- `:timeout-ms` is an optional positive integer.
- The body is stored as a zero-argument thunk and is not evaluated during file loading.
- Registration records the source location and registration index.
- Duplicate names in one file are a `duplicate-test-name` load error. Dynamic test
  generation must produce unique names.

Initial options reject unknown keys. This prevents a misspelled option from being ignored.

### Matchers and control operations

| Form | Contract |
|---|---|
| `(test/is expression [message])` | Record whether `expression` is truthy. |
| `(test/is= expected actual [message])` | Record structural equality. The expected value is first. |
| `(test/is-not= unwanted actual [message])` | Record structural inequality. |
| `(test/throws expression [expected])` | Record whether `expression` raises. An optional string requires a message substring; a regex requires a message match. |
| `(test/near expected actual tolerance [message])` | Record `abs(expected - actual) <= tolerance`. |
| `(test/contains container expected-member [message])` | Strings use substring containment; lists and vectors use structural element equality. |
| `(test/context label body ...)` | Add a non-empty assertion-context label while evaluating `body`. Contexts can nest. |
| `(test/require expression [message])` | Record a truthiness assertion. Abort only the current test if it fails. |
| `(test/fail [message])` | Record an unconditional failure and continue. |
| `(test/skip reason)` | Mark the current test skipped and stop its body. |

`test/contains` keeps the existing `sema-test` argument order: container first, expected
member second. `test/is=` keeps the existing builtin and package order: expected first,
actual second.

An assertion context is dynamically scoped. It is restored when its body returns, raises
a Sema error, or produces a runtime-root stop outcome. If an outer `try` catches an error
from a context body, later assertions do not retain that context.

All matchers capture the original form and invocation location. They evaluate each operand
exactly once. An error while evaluating an operand marks the test errored and stops that
test. It is not converted into a failed comparison. `test/throws` is the only initial
matcher that treats an error as its expected input.

Ordinary matchers record and continue. `test/require` records one failed assertion and
then stops the current test without changing its status to errored. `test/skip` is valid
only before the first assertion. A later call returns `skip-after-assertion` so a failure
cannot be hidden by a skip.

Calling a `test/*` recorder outside an active registered test returns
`no-active-test`. Calling `deftest` outside a test-worker interpreter fails because the
test DSL is not installed there.

### Macro expansion

The test DSL is a tracked Sema source file embedded unconditionally with `include_str!`
from the `sema-test-runner` crate. The worker evaluates it after normal interpreter setup.
It is not added to `crates/sema-eval/src/prelude.rs`.

Macros use generated bindings and must not capture user bindings. The expansions pass
quoted source forms, evaluated values, and invocation locations to hidden native
recorders. Simple matchers evaluate their operands normally before calling the recorder;
they do not ask a native function to re-enter the evaluator. `test/throws` uses a macro
expansion around `try` so it can record either the returned value or the caught error.

Phase 1 must prove that the original macro invocation span survives lowering. If the
current macro representation cannot supply it, add one internal compiler marker that
lowers to the current source span. Do not infer locations by searching source text.

### Runtime-root stop outcomes

`test/require`, `test/skip`, and `exit` must stop evaluation without using a catchable
`SemaError`. Add an internal runtime-root stop outcome with these cases:

- required assertion failed;
- test skipped with a reason;
- exit requested with an integer code.

The VM propagates this outcome to the runtime root. Sema `try` cannot catch it. The worker
records the outcome, retains output already captured by the root, and performs normal
root teardown. It then runs the next registered test when one remains. A legacy script
exit ends that file. After file work is complete, the worker sends `file-finished`, performs
interpreter teardown, and exits successfully as a protocol process.

Only a test-worker interpreter changes `exit` this way. Normal Sema execution keeps the
documented process-exit behavior. In a registered test, any requested exit becomes
`unexpected-exit`. In a legacy script test, requested exit zero passes and requested
nonzero exit fails. A direct native process termination remains possible as a fault; the
parent classifies it from the missing protocol completion.

## Registration and GC ownership

The worker owns one `Rc<TestRegistry>`. The registry contains registered test thunks,
metadata, the active test state, and assertion records.

Install hidden native functions such as `__test/register` and `__test/record` with a
typed `NativeFn.payload`. Register a payload tracer for `TestRegistry` because registered
thunks contain `Value` and can retain environments. Native callbacks use the typed
payload constructors and do not strongly capture the registry. Mark the registered thunk
argument as escaping.

Do not use an independent thread-local `Vec<Value>`. Multiple interpreters can exist on
one thread, and untraced values would violate the collector contract. Dropping the worker
interpreter and registry releases all registered thunks. Worker tests must prove that
repeated file runs do not increase the GC registry or retain old environments.

## Rust architecture

Add an internal workspace crate named `sema-test-runner`. This name distinguishes the
Rust implementation crate from the user-facing registry package named `sema-test`.

Using the repository's dependency notation, add these edges:

```text
sema-core / sema-reader / sema-eval / sema-vm
                       <- sema-test-runner
                       <- sema-lang

sema-test-runner <- sema-mcp <- sema-lang
```

Both `sema-lang` and `sema-mcp` depend on `sema-test-runner`. `sema-mcp` must not depend
on the `sema-lang` crate because `sema-lang` already depends on `sema-mcp`.

Suggested layout:

| File | Responsibility |
|---|---|
| `crates/sema-test-runner/src/lib.rs` | Public requests, results, limits, and top-level runner API. |
| `discovery.rs` | File conventions, ignores, canonicalization, deduplication, sorting, and filters. |
| `registry.rs` | `TestRegistry`, native recorder registration, payload tracing, and assertion state. |
| `worker.rs` | Test-mode interpreter setup, file loading, registered-test execution, and legacy script mode. |
| `protocol.rs` | Versioned parent/worker NDJSON messages. |
| `process.rs` | `current_exe()` worker launch, timeout, exit, signal, and crash handling. |
| `result.rs` | Serializable canonical result types and totals. |
| `report/pretty.rs` | Human reporter and bounded failure diffs. |
| `report/json.rs` | Canonical JSON serialization. |
| `report/junit.rs` | JUnit adapter added in Phase 4. |
| `report/dot.rs` | Dot adapter added in Phase 4. |
| `test_prelude.sema` | Tracked, embedded test DSL macros. |

Add `Interpreter::submit_call` in `sema-eval` if no existing host API can submit a stored
callable as a new runtime root. Each test thunk must run as its own root so it can suspend,
capture output, receive a timeout, and return a structured error without synchronous
host-only callback re-entry.

The runner library accepts a `RunRequest` and returns `RunResult`. Reporter selection stays
outside execution. The CLI and MCP tool never implement separate discovery or execution
logic.

The hidden worker bootstrap must run before normal command dispatch and before an embedded
program starts. Put its argument detection and startup code in a shared entry path used by
both the normal `sema` binary and executables produced by `sema build`. A built executable
that offers `--mcp` must be able to start its own worker through `current_exe()`. If an
executable cannot do that, its MCP server must not list `run_tests`.

## Discovery

Bare `sema test` searches from the current directory. The default conventions are:

- `**/*.test.sema`;
- `**/*_test.sema`;
- `**/tests.sema`.

Do not discover every `.sema` file under a `test/` or `tests/` directory. Such directories
contain helpers such as `harness.sema`. Do not discover `test.sema`; `sema-coder/test.sema`
is itself a runner and would recurse.

Default recursive discovery does not enter `.git`, `.sema`, `target`, or `node_modules`,
and does not follow directory symlinks. An explicit file path can select any `.sema` file
after normal path and sandbox checks. An explicit directory uses the same file conventions
and ignore rules. CLI glob arguments are accepted and use the same normalization.

Discovery performs these steps in order:

1. Expand explicit paths and globs or apply the default conventions.
2. Reject non-UTF-8, non-regular, and non-`.sema` files.
3. Check the user path and canonical target against the allowed-path policy.
4. Canonicalize and deduplicate files.
5. Sort by normalized relative path.
6. Apply the file-count and path-length limits.

No discovered files is `no-tests-discovered`. Files were discovered but no registered or
script tests remain after filters is `no-tests-selected`. Both are runner errors unless
`allow_empty` is true. An allowed empty run returns success with zero totals and a warning.

## Legacy script-test compatibility

The migration rules are limited to the current conventions:

- A `tests.sema` or `*_test.sema` file that registers tests uses registered-test mode.
- If such a file registers no tests, its file evaluation is one script test.
- A `.test.sema` file that registers no tests returns `no-tests-registered`.
- `--require-registered-tests` disables script mode for all files.

For a script test, requested exit zero or normal completion passes; requested nonzero exit
fails; a timeout, signal, panic, or unreported process exit does not pass. Malformed worker
protocol is a runner error, not a script-test result. Captured output belongs to the one
script test. The result records `mode: "script"` so CI and agents can identify remaining
legacy files.

If a registered test calls `exit`, it never passes even when the code is zero. The worker
uses the runtime-root exit outcome and marks the current test `unexpected-exit`. Tests not
yet run can continue because the worker process did not terminate. If native code
terminates the worker directly, the parent marks the current test `crashed` and the
remaining tests `not-run`.

The worker sends and flushes the registered-test manifest before it starts the first test.
It sends and flushes one result after every test. A later crash therefore preserves all
completed results and identifies the tests that did not run.

## Process and worker contract

The parent starts the hidden worker from `std::env::current_exe()`. It must not search
`PATH`, shell out through `sh` or `cmd`, or call a development tool. The hidden command is
not shown in normal CLI help. The same early bootstrap works in the normal `sema` binary
and in a built standalone executable that exposes MCP.

The parent and worker use newline-delimited JSON messages with an internal protocol
version. Initial message kinds are:

```text
hello
file-started
file-loaded
test-started
output
test-finished
file-finished
```

Every line is flushed. `file-loaded` contains the complete ordered test descriptor list.
The parent rejects a missing `hello`, unsupported version, invalid transition, duplicate
completion, oversized line, or trailing invalid JSON as `worker-protocol-error`.

The NDJSON protocol uses a dedicated parent-owned pipe or operating-system handle. Do not
use worker stdout for protocol data. Pass only the required endpoint to the worker, and
prevent a test subprocess from inheriting it.

Add a worker-only runtime output sink. Each Sema stdout or stderr write sends an `output`
event with its phase or test ID, stream, sequence number, and text, then flushes the event.
This preserves output emitted before a controlled exit or later process crash. The parent
applies output limits and builds each ordered `OutputChunk` list from these events.
The sink splits large writes before JSON encoding so an individual output event cannot
exceed the protocol line limit.

The parent also captures raw worker stdout and stderr. These streams contain output that
bypasses the runtime sink, such as panic, native-library, or inherited subprocess output.
Store it as bounded worker diagnostics in parent-observed arrival order. It cannot corrupt
the protocol and is never forwarded directly as reporter output. Protocol, captured
output, and diagnostic buffers have separate hard byte limits.

One worker handles one file and then exits. The runner is sequential in the initial
implementation. Parallel workers require a later decision about shared cassette files,
ports, databases, and reporter order.

### Timeouts

- Default registered-test timeout: 60,000 ms.
- `deftest :timeout-ms` overrides the per-test value.
- The CLI and MCP request can set a run-wide default.
- Default hard worker timeout: 300,000 ms, including file loading and teardown.
- A watchdog cancels a live runtime root when possible. The parent terminates the worker
  if it does not stop within a bounded grace period.

A worker timeout during file loading is a file error. A timeout during a registered test
marks that test `timed-out` and later tests `not-run` if the worker must be terminated.
Timeouts make the run unsuccessful.

## Isolation and sandbox policy

Tests in one registered file share the same interpreter, globals, module cache, working
directory, and process environment. Run them in registration order. File processes do not
share interpreter state.

CLI runs use the normal CLI sandbox policy. An unrestricted CLI invocation remains
unrestricted. MCP runs inherit the MCP interpreter's denied capabilities and allowed
paths. Starting an internal worker does not grant `Caps::PROCESS` to test code.

Derive a serializable worker policy from `Sandbox`. Pass it to the worker through a
parent-owned bootstrap input that the worker reads before Sema evaluation. Do not expose
the hidden worker arguments or policy path through `sys/args`. Reconstruct the sandbox and
create the worker interpreter with `Interpreter::new_with_sandbox`; never use
`Interpreter::new()` for an MCP run.

The parent checks every discovered path before starting a worker. The worker checks the
canonical file again before reading it. Tests retain the caller's `FS_READ`, `FS_WRITE`,
`SHELL`, `NETWORK`, `ENV_READ`, `ENV_WRITE`, `PROCESS`, `LLM`, and `SERIAL` restrictions.

The worker bootstrap and protocol are host implementation details. Their private files or
pipes are not added to allowed paths for test code.

## Filters, tags, and skips

The full test name is displayed as:

```text
relative/path.test.sema > test name
```

`test_filter` is a case-sensitive substring match against this full name. Paths select
files; `test_filter` selects registered tests inside the loaded files. Filtering occurs
after file loading because evaluation performs registration. Top-level file code can
therefore run even when all registered tests are filtered out.

`test/context` labels are created only while a selected test runs. They cannot take part
in pre-run selection. A failed assertion uses this breadcrumb:

```text
relative/path.test.sema > test name > outer context > inner context
```

Tags are keywords stored without the leading colon in JSON and CLI arguments. A test must
match all requested include tags and no excluded tag. The runner adds an implicit `live`
exclusion when `include_tags` does not contain `live`. Including `live` removes only that
implicit exclusion. An explicit `exclude_tags: ["live"]` still excludes the test. Sandbox
capabilities still apply after tag selection.

A conditional test uses `test/skip` so the result includes the test and reason. Do not
print a skip message and record a passing assertion. A skipped test counts as selected and
does not make the run unsuccessful.

## Canonical result model

All user-facing JSON fields use snake_case. Enum values use kebab-case strings. Sema names
use slash namespaces and keyword values if the result is later exposed in-language.

```text
RunResult {
  schema_version,
  success,
  root,
  duration_ms,
  totals,
  files,
  warnings
}

RunTotals {
  files,
  tests,
  passed,
  failed,
  errored,
  skipped,
  timed_out,
  unexpected_exits,
  crashed,
  not_run,
  load_errors,
  assertions
}

FileResult {
  path,
  mode,
  status,
  duration_ms,
  load_error?,
  tests,
  output,
  output_truncated,
  output_bytes_omitted,
  worker_diagnostics,
  worker_diagnostics_truncated,
  worker_diagnostic_bytes_omitted,
  worker_exit?
}

TestResult {
  id,
  name,
  full_name,
  registration_index,
  tags,
  status,
  duration_ms,
  assertions,
  skip_reason?,
  error?,
  output,
  output_truncated,
  output_bytes_omitted
}

AssertionResult {
  index,
  kind,
  status,
  context_path,
  form,
  expected?,
  actual?,
  message?,
  location
}

OutputChunk { stream, text }
Location { path, line, column, end_line, end_column }
TestError { kind, message, location?, stack_trace? }
WorkerExit { code?, signal?, kind }
```

`status` values for tests are `passed`, `failed`, `errored`, `skipped`, `timed-out`,
`unexpected-exit`, `crashed`, and `not-run`. File status is derived from loading, contained
tests, and worker completion. A script file contains exactly one `TestResult`.

`mode` values are `registered` and `script`. File `status` values are `passed`, `failed`,
`errored`, `timed-out`, and `crashed`. Derive file status in this priority order: worker
crash, timeout, load or test error, failed assertion or unexpected exit, then passed. A
file with only passed or skipped selected tests passes. `RunResult.success` is true only
when every file passes, or when an explicitly allowed empty run completes.

Worker diagnostics use `OutputChunk` values but are separate from Sema output because exact
cross-stream program order and test attribution are not available for writes that bypass
the runtime sink.

Expected and actual values are deterministic Sema renderings plus their Sema type names.
Do not require every `Value` to have a JSON representation. Diffs operate on the rendered
text and are bounded.

Output is an ordered list of stdout and stderr chunks. The default retained output cap is
256 KiB per test and 256 KiB for file loading. When the cap is exceeded, preserve the
prefix and suffix, set `output_truncated`, and report the omitted byte count. Reporter and
MCP response limits can apply a second bounded summary without changing the stored test
status.

Use monotonic time for durations. Duration values are data but are normalized out of
snapshot oracles.

## CLI

```text
sema test [PATHS...]
  [--test-filter <substring>]
  [--include-tag <tag>]...
  [--exclude-tag <tag>]...
  [--timeout-ms <n>]
  [--worker-timeout-ms <n>]
  [--reporter pretty|json|dot|junit]
  [--output <path>]
  [--show-output]
  [--allow-empty]
  [--require-registered-tests]
  [--update-cassettes]
  [--no-color]
```

Do not add `--all`. Explicit paths already permit unusual suites, while recursive execution
of every `.sema` file can run application entry points and other top-level side effects.

Reporter output goes to stdout unless `--output` is present. Runner diagnostics go to
stderr. Captured test output never writes directly to either parent stream. JSON and JUnit
stdout contain only their selected format. ANSI is used only by the pretty reporter on a
TTY and is disabled by `--no-color`.

The default reporter is always `pretty`. TTY detection changes color only; it does not
silently change the data format.

Phases 0 through 3 accept `pretty` and `json`. Phase 4 adds `dot` and `junit`. Phase 5 adds
`--update-cassettes`. The command synopsis above is the complete target interface.

Exit codes:

- `0`: at least one selected test completed or skipped, and none failed, errored, timed
  out, exited unexpectedly, or crashed; or an allowed empty run completed.
- `1`: the run completed with a test failure, test error, load error, timeout, unexpected
  exit, or crash.
- `2`: invalid CLI arguments, denied path, discovery error, no tests without
  `--allow-empty`, reporter write failure, or worker protocol failure.

## Reporters

Every reporter consumes the final `RunResult`. No reporter decides test status.

### Pretty reporter

- Print one compact row for each file.
- Collapse passing test details by default.
- Print a failure block with the failure breadcrumb, message, source location, code frame,
  expected/actual values, and bounded diff.
- Print captured output only for failed, errored, timed-out, unexpectedly exited, or
  crashed tests unless `--show-output` is set.
- Apply the same display rule to worker diagnostics. Always include available worker
  diagnostics in a worker-crash block.
- Distinguish assertion failure, uncaught error, timeout, unexpected exit, load error, and
  worker crash with text as well as color.
- Print separate file, test, and assertion totals.

### JSON reporter

Serialize the canonical `RunResult` directly. Do not imitate a partial Jest or Vitest
schema. The `schema_version` field provides the compatibility contract.

### Dot and JUnit reporters

The dot reporter emits one character per test and a final summary. The JUnit reporter maps
failed tests to `failure`; errors, timeouts, unexpected exits, crashes, and load errors to
`error`; and skipped or not-run tests to `skipped`. Reporter snapshot tests consume a fixed
`RunResult`; they do not execute Sema code.

HTML remains deferred until there is a concrete consumer.

## MCP `run_tests`

Add one MCP tool backed by the same `RunRequest` and `RunResult` types.

The MCP host calls the shared runner library. Its child executable is always the MCP host's
`current_exe()`. This applies to `sema mcp` and to `--mcp` in a built standalone
executable. Tool listing must omit `run_tests` if the host does not provide the hidden
worker bootstrap.

Complete argument schema:

```json
{
  "paths": ["tests"],
  "test_filter": "parser",
  "include_tags": ["offline"],
  "exclude_tags": ["live"],
  "timeout_ms": 60000,
  "worker_timeout_ms": 300000,
  "allow_empty": false,
  "require_registered_tests": false,
  "update_cassettes": false
}
```

All fields are optional. Reject unknown fields and invalid combinations. Paths are resolved
against the configured MCP working directory and checked against the inherited sandbox.
Phases 0 through 4 omit `update_cassettes`; Phase 5 adds it.

Return the canonical result as `structuredContent` with an exact `outputSchema`. Provide a
short text summary for clients that only display text content. Never include ANSI.

MCP error rules:

- Every completed run returns `isError: false`, including a run with non-passing tests.
  Failed, errored, timed-out, unexpectedly exited, crashed, and load-error results set
  `success: false`. Skipped tests alone do not make a run unsuccessful.
- Invalid arguments, denied paths, discovery failure, bootstrap failure, and worker
  protocol failure return `isError: true` with a structured runner error.
- A test load error is part of a completed run and returns `isError: false`.

The MCP response applies a bounded result size. If detailed assertions or output exceed
the limit, retain totals and every non-passing test summary, truncate passing detail first,
and report truncation metadata. Do not return a successful empty object after truncation.

## LLM cassette testing

Tests use the existing `llm/with-cassette` or `llm/cassette-load` APIs. The test worker
installs an explicit cassette policy:

- Default test policy is replay-only. A declared `:auto` cannot make a provider call during
  a normal test run.
- Tests tagged `:live` are excluded unless explicitly included.
- `--update-cassettes` is the only runner flag that permits cassette refresh.
- MCP also requires inherited `LLM`, `NETWORK`, and `FS_WRITE` permission for refresh.

The current cassette `:record` mode appends entries, while lookup returns the first entry
for a key. Appending a second entry therefore does not update replay. Do not implement
`--update-cassettes` as a simple mode override.

Phase 5 adds a real update operation: replace an entry by `(kind, key)`, remove duplicates,
and atomically rewrite the tape after successful recording. Keep the original tape if the
test or write fails. Sequential file workers avoid cross-worker writes in the initial
implementation; still use a per-path process lock so another Sema process cannot update
the same tape concurrently.

Any change to the agent loop, retry behavior, cassette policy, or provider request path
requires a deterministic `FakeProvider` test in addition to cassette integration tests.

## Existing `sema-test` package and migration

Do not remove the registry package as part of the runner implementation. It remains useful
for older Sema versions and current suites during migration.

The builtin DSL deliberately uses different matcher names from the package's
`test/assert-*` functions. The package functions are fail-fast and return `#t`; silently
changing them to report-and-continue would be incompatible.

Migration sequence:

1. Ship native script-test support and prove it against representative copies of the
   current `sema-coder` and package suites.
2. Update the package template to use `deftest` and `test/*` matchers without
   `sema pkg add sema-test`.
3. Convert `sema-coder` files from `check`/`done` to registered tests while keeping the
   existing runner available for comparison.
4. Convert package suites in batches. Preserve their explicit test order and replace
   printed live-test skips with tags or `test/skip`.
5. Release a final `sema-test` package version whose README points new users to
   `sema test`. Keep its implementation for compatibility.
6. Remove legacy script mode only in a later breaking release and only after repository
   search shows no maintained suite depends on it.

The runner reports script mode in JSON and in one pretty warning per run. This provides a
measurable migration state without failing existing suites.

## Implementation sequence

### Phase 0: fixtures and contracts

1. Copy small representative fixtures from `sema-coder` and `sema-test` behavior into
   `crates/sema-test-runner/tests/fixtures` without depending on sibling repositories.
2. Record golden behavior for child exit, ordered tests, conditional live tests, quiet
   success, failure output, and the current expected-first equality order.
3. Add serde round-trip tests for the canonical result and worker protocol types.
4. Fix the public names, limits, exit codes, status values, and discovery rules from this
   plan as test constants.

Checkpoint: result types and fixture expectations compile before interpreter integration.

### Phase 1: registry and test DSL

1. Add `sema-test-runner` to workspace members, default members, workspace dependencies,
   internal exact-version pins, package scripts, and release configuration.
2. Implement `TestRegistry`, its payload tracer, hidden native recorders, and teardown
   tests.
3. Add the tracked embedded `test_prelude.sema` with hygienic `deftest` and `test/*`
   macros.
4. Add source-form and invocation-location capture tests.
5. Add `Interpreter::submit_call` or the equivalent runtime-root API for stored thunks.
6. Add non-catchable runtime-root outcomes for required-failure, skip, and test-worker
   exit.
7. Add the worker-only output sink and dynamic assertion-context cleanup.
8. Run registered tests sequentially in one test-mode interpreter.

Checkpoint:

```bash
cargo nextest run -p sema-test-runner
cargo nextest run -p sema-eval
```

Tests cover multiple failures, fatal requirements, skips, nested assertion contexts,
dynamic registration, duplicate names, operand errors, asynchronous tests, and registry
teardown.

### Phase 2: worker, discovery, CLI, pretty, and JSON

1. Implement deterministic discovery and explicit path handling.
2. Add the hidden one-file worker command, the shared early bootstrap, the dedicated
   protocol pipe, and the versioned NDJSON protocol.
3. Start workers with `current_exe()` from both normal and built executable hosts. Implement
   timeouts, exit classification, output limits, and partial-result recovery.
4. Implement automatic legacy script mode and `--require-registered-tests`.
5. Add the `sema test` CLI command, pretty reporter, JSON reporter, and exit codes.
6. Add fixture-based CLI integration tests through the real binary.

Checkpoint: the native runner executes representative registered, `sema-coder`, and
package-style suites without reading or parsing their human summary text.

### Phase 3: MCP and sandbox propagation

1. Add serializable worker policy derivation from `Sandbox`.
2. Enforce user-path and canonical-path checks in parent and worker.
3. Add `run_tests` to MCP with exact input and output schemas.
4. Add structured-result truncation that preserves all non-passing summaries.
5. Test tool listing and calls through `sema mcp` and a built standalone executable
   launched with `--mcp`. Confirm that each host starts the hidden worker from its own
   `current_exe()`.

Checkpoint: denied file, network, write, process, environment, LLM, and serial operations
remain denied inside the worker, while an allowed restricted suite runs successfully.

### Phase 4: JUnit and dot reporters

1. Add reporter adapters over the canonical result.
2. Add snapshot tests with fixed durations and paths.
3. Verify XML escaping, skipped tests, errors, timeouts, and bounded output.

### Phase 5: cassette update and agent suite

1. Add the replay-only test policy.
2. Add atomic replace-by-key cassette update semantics.
3. Add `--update-cassettes` to CLI and MCP permission checks.
4. Add a checked-in tool-using agent suite recorded once and replayed in CI.
5. Add required `FakeProvider` coverage.

### Later phases

Consider watch mode, bounded parallel workers, coverage, TAP, custom reporters,
property-based testing, and HTML only after the planned implementation has usage data.

## Effort estimate

For one coding agent working to review-ready quality, use these focused-work estimates:

| Work | Estimate |
|---|---:|
| Phase 0: fixtures and fixed contracts | 1–2 agent-days |
| Phase 1: registry, macros, VM root outcomes, output sink | 3–5 agent-days |
| Phase 2: process protocol, discovery, CLI, pretty, JSON | 3–5 agent-days |
| Phase 3: MCP, sandbox propagation, built executable path | 2–4 agent-days |
| Phase 4: JUnit and dot reporters | 1–2 agent-days |
| Phase 5: cassette update and agent suite | 2–4 agent-days |
| Package-boundary gate, docs, and final integration fixes | 1–3 agent-days |

The useful CLI runner through Phase 2 is approximately 7–12 agent-days. The complete
planned implementation is approximately 13–25 agent-days. This estimate includes tests
and review corrections. It excludes conversion of all maintained sibling-project suites;
budget another 3–8 agent-days for that migration after the compatibility path is proven.

The largest uncertainties are the non-catchable VM root outcome, reliable output delivery
before a hard worker failure, and hidden-worker startup in built standalone executables.
These items must be proved before reducing the estimate.

## Test matrix

### DSL and registry

- hygienic expansion with conflicting user binding names;
- form and source-location capture;
- operands evaluated once;
- multiple independent failures continue;
- `test/require` stops only the current test;
- `test/skip` before assertions and rejected after assertions;
- `try` cannot catch required-failure, skip, or test-worker exit outcomes;
- uncaught error differs from assertion failure;
- `test/throws` with any error, substring, regex, and no error;
- exact tolerance boundary for `test/near`;
- string, list, and vector containment;
- nested assertion contexts and unique test names;
- assertion context is restored after a caught error;
- dynamic registration order;
- duplicate test names rejected;
- no active test errors;
- traced thunk retention and release across repeated workers.

### Discovery

- `.test.sema`, `*_test.sema`, and `tests.sema`;
- `harness.sema` and `test.sema` not discovered;
- ignored directories;
- explicit file, directory, and glob inputs;
- canonical deduplication;
- stable path sort;
- file and path limits;
- symlink file and directory behavior;
- no files and no selected tests;
- allowed empty run.

### Worker isolation

- fresh state across files;
- shared state and order within a file;
- registered test requests `exit 0` and `exit 1` without losing output or protocol data;
- legacy script exits 0 and 1;
- direct native process termination remains contained by the parent;
- panic or signal termination;
- load error before registration;
- file-load timeout and test timeout;
- detached async task teardown;
- subprocess, watcher, terminal, database, and other interpreter teardown hooks;
- partial results survive a later crash;
- flushed output survives a later crash;
- later tests become `not-run`;
- worker protocol rejects malformed or oversized input;
- raw stdout containing valid-looking NDJSON cannot alter the protocol;
- test subprocesses do not inherit the protocol endpoint.

### Output and reporters

- interleaved stdout and stderr order;
- output during file loading and during a test;
- bounded raw worker diagnostics remain separate from captured Sema output;
- passing output hidden by default;
- failure output shown;
- output cap prefix, suffix, and omitted-byte count;
- JSON stdout has no reporter text or ANSI;
- pretty output distinguishes every non-passing status;
- scalar expected/actual output and multiline diff;
- code frames at first, middle, and last lines;
- JSON schema round trip and version rejection.

### CLI

- exit codes 0, 1, and 2;
- default pretty reporter independent of TTY;
- color auto-detection and `--no-color`;
- output file success and write failure;
- test-name and tag filtering;
- default exclusion of `:live`;
- explicit inclusion of `live` and precedence of an explicit `live` exclusion;
- timeout overrides;
- strict registered-test mode;
- invocation from outside the project directory;
- child executable resolution through `current_exe()` with no `sema` on `PATH`;
- early worker bootstrap in a built standalone executable.

### MCP and sandbox

- unknown and invalid arguments;
- relative, absolute, traversal, and symlink paths;
- allowed and denied canonical targets;
- every inherited capability remains enforced;
- failed tests return `isError: false` with `success: false`;
- load errors return a completed structured run;
- permission and protocol errors return `isError: true`;
- no ANSI;
- bounded structured output retains all failure summaries;
- built standalone `--mcp` lists and runs `run_tests` through its own worker bootstrap;
- identical canonical results through CLI JSON and MCP, excluding host-only metadata.

### LLM

- normal tests force replay for declared `:auto` cassettes;
- cassette miss is a test error with no provider call;
- `:live` tests are excluded by default;
- update requires explicit flag and permissions;
- update replaces the first key, removes duplicates, and preserves unrelated entries;
- failed recording leaves the original tape unchanged;
- replay reports deterministic usage without a network call;
- FakeProvider covers tool-call correlation and failure paths.

## Packaged-binary regression gate

The test DSL is an embedded runtime input and the worker starts a companion invocation of
the current executable. Add `scripts/test-packaged-sema-test.sh` or extend the general
package-boundary harness with equivalent coverage:

1. Build the real `sema-lang` `.crate` and all unpublished local package inputs required
   by the test.
2. Rebuild from unpacked package contents with the checkout absent from runtime paths.
3. Put no checkout binary or development tool on `PATH`.
4. Run a passing registered suite and a failing registered suite.
5. Prove the embedded test DSL loads.
6. Prove the hidden worker resolves through `current_exe()`.
7. Build a small standalone executable from packaged inputs, start it with `--mcp`, and
   prove that `run_tests` uses that executable's hidden worker bootstrap.
8. Prove JSON output and exit status.

Add the gate to `.github/workflows/verify.yml` before release. A normal workspace test is
not sufficient because it can read files that a packaged crate omitted.

## Acceptance criteria

The planned implementation is complete when all of these statements are true:

1. A fresh installed `sema` runs a registered suite with no package installation.
2. Two files cannot share interpreter state.
3. Tests in one file run in documented registration order.
4. A test that calls `exit`, hangs, or crashes cannot terminate or hang the parent runner.
5. Current `tests.sema` and `*_test.sema` script suites can run during migration.
6. A registered file with no tests cannot pass silently.
7. Pretty output is quiet on success and complete on failure.
8. JSON and MCP use the same versioned result model.
9. Test failures are not MCP execution errors.
10. MCP workers preserve sandbox capabilities and allowed paths.
11. Normal test runs make no cassette provider call when replay data is missing.
12. Normal and built standalone MCP hosts start workers from their own executable.
13. The packaged-binary gate passes without the checkout or a `sema` binary on `PATH`.

## Risks and controls

| Risk | Control |
|---|---|
| Process startup makes many small files slow | Measure the current 29-file `sema-coder` suite; keep sequential execution first and add bounded parallelism only with evidence. |
| Shared state makes tests order-dependent within one file | Specify registration order, isolate files, and encourage helpers that restore environment and files. |
| Legacy script mode hides missing registration | Limit it to `tests.sema` and `*_test.sema`, mark results as `script`, warn once, and provide `--require-registered-tests`. |
| Native code terminates a worker before it sends a result | Use the last flushed protocol state to mark the current test `crashed` and later tests `not-run`; the test-worker `exit` builtin uses a runtime-root outcome instead. |
| A test writes unbounded output | Enforce per-test, file-load, protocol, reporter, and MCP byte limits. |
| Native or subprocess stdout corrupts the worker protocol | Use a dedicated inherited protocol endpoint, prevent inheritance by test subprocesses, and capture raw worker streams separately. |
| A worker hangs in native or external code | Use root cancellation plus a hard parent process timeout and bounded termination grace. |
| Test thunks create an uncollectable environment cycle | Store them in a registered traced payload and keep native closures free of strong `Value` or `Env` captures. |
| MCP starts an unrestricted interpreter | Serialize the inherited policy and require `Interpreter::new_with_sandbox` in the worker. |
| A built standalone MCP host cannot start the normal CLI's hidden command | Run worker bootstrap before embedded-program dispatch and test `run_tests` through the built executable. |
| Discovery runs helper or application files | Use exact file conventions, ignored directories, no directory-symlink traversal, and explicit-path checks. |
| Reporter logic changes test status | Compute status once in the canonical result before any reporter runs. |
| Cassette update appends an ineffective duplicate | Implement replace-by-key with atomic rewrite; never map update directly to current record mode. |
| The shipped crate omits the test DSL | Unconditional tracked embedding plus the packaged-binary regression gate. |
