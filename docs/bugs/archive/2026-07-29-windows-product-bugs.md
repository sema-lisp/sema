> ARCHIVED 2026-07-29: all product bugs fixed except the MCP HTTP cancel
> teardown, tracked with the other leftovers as WIN-1 in docs/deferred.md.
> UPDATE 2026-08-11: bug 5 below shipped a fix (`run_transport_task`,
> 2026-07-29) but is not fully closed — a residual low-frequency race
> remains on Windows. Reopened as WIN-1 in docs/deferred.md.

# Windows product bugs surfaced by the test-porting wave (2026-07-29)

The Windows test leg's first honest run (nightly harness, PR #135) reduced 189
failures to a handful of root causes. Most were test-portability issues, fixed
in the same PR. These five were REAL product bugs on Windows; none reproduce
on Unix. **Bugs 2, 3, and 4 are fixed in this PR's wave B** (tarball
`has_root` rejection; fmt separator normalization; rollback via a write-mode
handle) with their detector tests re-enabled cross-platform. Bugs 1 and 5
remain open; their detector tests stay red on Windows (or cfg-gated with a
pointer here) until fixed.

## 1. `sema build` executables never find their embedded payload (severity: high)

**FIXED — confirmed on Windows CI (run 30411862087: all 12 detector tests
green; FindResourceW returns valid handles; probe exes execute their embedded
programs).** A `sema build` `.exe`
booted as the plain sema CLI/REPL — `try_run_embedded()`
(`libsui::find_section("semaexec")`) returned `None`. Diagnosis chain: the
resource was structurally present (pefile parses it) but `FindResourceW`
failed with ERROR_RESOURCE_TYPE_NOT_FOUND (1813) — the PE resource directory
was serialized in *insertion* order, while the Win32 API's binary search
requires the spec's sorted order (named-before-ID, ascending). Both writers
in the old pipeline emit unsorted trees: libsui 0.16's own writer, and
editpe 0.1 (IndexMap insertion order). editpe 0.2 serializes sorted
(resource.rs `sorted_keys`), so the fix is: bump editpe to 0.2 and keep
`set_windows_version_info` as the FINAL pass — it re-serializes the whole
tree sorted, and payload + icons + VERSIONINFO all survive API-visible.
Verified structurally on a cross-built exe: root type IDs [3, 10, 14, 16] in
file order (sorted), RT_RCDATA/SEMAEXEC + icons + RT_VERSION present. This
affected every release to date; not a regression from the test wave.
Detected by: 10 `sema build` integration tests + the run-step of
`output_into_existing_directory` (integration_test.rs) + mcp_suite's
`standalone_binary_mode` (spawns a `sema build` binary as an MCP server).

## 2. `extract_tarball` path-escape: rooted driveless entries (severity: high, security)

crates/sema/src/pkg.rs:1319 rejects absolute tar entries via
`path.is_absolute()`. On Windows, a rooted driveless entry like
`/tmp/pwned.txt` is NOT `is_absolute()`, passes the `ParentDir` scan, and
`dest.join(...)` re-roots it onto dest's drive — writing `C:\tmp\pwned.txt`
OUTSIDE the extraction dir. Fix: reject `path.has_root()` (any
`Component::RootDir`/`Prefix`), not just `is_absolute()`.
Detected by: `extract_tarball_rejects_absolute_paths` (pkg.rs tests; the test
now uses a host-absolute entry so it guards the primary property on all
platforms — the driveless-rooted case still needs the product fix + a
dedicated test).

## 3. `sema fmt` non-glob ignore entries never match (severity: medium)

`is_ignored` in `run_fmt` (crates/sema/src/main.rs:3608-3619) compares
normalized `/`-separated ignore prefixes against walked paths that use `\` on
Windows, so literal-prefix entries (`vendor/`) are silently ignored while glob
entries happen to work (the `glob` crate matches either separator). A Windows
user's sema.toml `[fmt] ignore` prefixes do nothing. Fix: normalize the
candidate path's separators before the prefix compare.
Detected by: `fmt_ignore_list_skips_globs_and_prefixes`,
`fmt_check_respects_ignore_list` (misc_suite fmt_cli_test).

## 4. Memory-store flush rollback is a no-op on append handles (severity: medium)

`write_lines` (crates/sema-stdlib/src/memory.rs ~400) opens the JSONL sidecar
with `.append(true)` and rolls back a failed write via
`file.set_len(pre_len)`. Windows append handles carry `FILE_APPEND_DATA`
without `FILE_WRITE_DATA`, so the truncate fails silently and a torn line
survives (CI observed `{"con{"content":"turn-two"...}`). Fix: reopen with
write access (or open read+write and seek to end) for the rollback path.
Detected by: `memory_partial_flush_failure_retries_without_duplicates`
(llm_suite memory_test; `#[cfg(unix)]` with a pointer here).

## 5. Cancelling `mcp/close` over HTTP doesn't sever the transport (severity: low)

On Unix, dropping the in-flight request future closes the connection and the
peer sees the disconnect; on Windows the peer observed no disconnect within
30s (`interruptible_blocking` select path in crates/sema-mcp/src/builtins.rs;
Http `shutdown` in client.rs). Needs Windows-side investigation of
reqwest/hyper teardown semantics when the op future is dropped.
Detected by: `runtime_mcp_close_wait_is_promptly_cancellable`
(mcp_runtime_test; `#[cfg(unix)]` with a pointer here).

**PARTIALLY FIXED 2026-07-29, REOPENED 2026-08-11**: `run_transport_task`
(`crates/sema-mcp/src/builtins.rs`) moved the drop of the in-flight request
future onto a task spawned on the shared multi-thread Tokio runtime instead
of a plain blocking thread, so the connection's hyper dispatcher task
reliably gets a chance to notice the abandoned request and close the socket
— green on Windows CI at the time (close, call, and connect detectors all
passing). But this is best-effort, not a hard guarantee: hyper has no API to
force-close a specific pooled connection from outside
(hyperium/hyper#3533) — the fix only ensures a wake is attempted, not that
the resulting poll completes before an external observer checks. Nightly run
30979939966 (2026-08-05) reproduced the original symptom on
`runtime_mcp_call_http_wait_is_promptly_cancellable`: clean Sema-side
teardown (`live_tasks=0 resource_gates=0 cancel_accepted=true`) but the peer
saw no disconnect within 30s. Reopened as WIN-1 in `docs/deferred.md`. The
durable fix likely needs the HTTP transport to drive `hyper::client::conn`
directly (holding the `Connection` future so cancel can abort it explicitly)
instead of `reqwest::Client`'s pooled connections, which don't expose that
control.

## Test-infra debt (not product)

- **FIXED (#136, leg fully green 7193/7193):** `tests/common/watchdog.rs`
  `BoundedDrain::finish` cancelled reads with `CancelSynchronousIo`, but
  std's child-stdio pipes are overlapped named pipes it can never cancel;
  now targets the FILE with `CancelIoEx` on a handle captured before the
  reader moves into the drain thread.
- `stdio_server_exited` (mcp_runtime_test.rs) probes liveness with python
  `os.kill(pid, 0)` + `ps`: on Windows `os.kill(pid, 0)` KILLS the process and
  `ps` doesn't exist, so the probe reports "exited" vacuously.
- Checkout CRLF: git's text heuristic + autocrlf mangles all-ASCII binary-ish
  fixtures (the 751-byte PDF; the workflow golden journal). Scoped
  `crates/sema/tests/fixtures/.gitattributes` (`*.pdf binary`) landed with the
  wave; a broader root `.gitattributes` (`*.sema text eol=lf`, `*.sh text
  eol=lf`, `fixtures/** -text`) is the durable fix and still open.
