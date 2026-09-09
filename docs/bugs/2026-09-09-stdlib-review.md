# Stdlib review — 2026-09-09

## Scope and result

Ten `gpt-5.6-terra` reviewers examined separate stdlib areas after the three
previously reported bugs were fixed. The parent checked the reports against
source, contracts, and small local CLI examples. This was a functional review,
not a security scan or an exhaustive proof of correctness.

The review retained 23 new findings: initially 13 had local execution evidence
and 10 had source-level evidence only. The follow-up fix pass verified all 23
against their contracts and added bounded regression coverage. All 23 now have
implementation fixes. The original evidence and source locations below describe
the code before this fix pass, not the current line numbers.

Tests use private temporary files, local Git fixtures, loopback servers, fake
providers, and injected backend failures. No external APIs, hardware, or user
data are used. Potentially unbounded allocations and loops were not executed
against unfixed code; admission guards were installed first.

The preceding fix pass also covered:

- Checked stepping in `i64-array/range` at both integer limits.
- Shared map-key validation in `list/group-by`, `list/key-by`, and `frequencies`,
  including callbacks that suspend and rejection before later callbacks run.
- ANSI-aware display truncation and wrapping. Escape sequences are not split;
  combining sequences are measured without intervening ANSI codes. Truncation
  retains closing style controls but discards unrelated commands in omitted
  text. Existing mutable-key examples now freeze their keys.

## Preceding fix pass verification

- Initial focused run: 71 passed. After the ANSI follow-up and frozen-key
  fixture updates, a further focused run passed all 32 selected tests.
- Final full run: `cargo nextest run --no-fail-fast --test-threads 4` —
  **7,907 passed, 57 skipped**, 131.545 seconds. Display-only status flags were
  used to reduce output.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo fmt --all -- --check`, `git diff --check`, and
  `scripts/check-unified-runtime-legacy.sh` passed.
- Default-concurrency `jake test` runs exposed existing Ctrl-C fixture timing
  failures in `sigint_tears_down_parked_subprocess` and
  `sigint_cancels_long_running_sleep_promptly`. The former passed in isolation;
  both passed in the final four-worker full run. These fixtures use fixed
  startup delays and process-name polling. They were not changed in this pass,
  and the lower-concurrency pass does not establish that their timing is robust.
- Concurrent LSP/DAP and other test-harness edits were left untouched. No
  commits or external writes were made by this pass.

## Follow-up fixes for all 23 findings

| # | Fix | Regression coverage |
| --- | --- | --- |
| 1 | Map lowercase match offsets back to whole source characters. | Expanding/shrinking case mappings, partial expansion matches, contextual Greek sigma, Unicode radius, empty query. |
| 2 | Compare real numeric keys numerically, with stable equal keys and NaNs last among numbers. | Mixed int/float/rational and mixed-type keys; suspending key callback. |
| 3 | Apply target EOF-newline metadata. | 147 small old/new/context round trips, including empty files and blank lines. |
| 4 | Write raw stdout/stderr bytes; reject invalid UTF-8 on text capture routes. | Real CLI byte output, root capture, host capture, no replacement characters. |
| 5 | Roll back insert/update/delete on persistence failure. | Both ABIs, cumulative size rejection, live/reopened contents, injected invalid parent path. |
| 6 | Reject reopening checked-out databases; retain checkout interrupt identity. | Held checkout, busy reopen, permitted idle reopen, retired ownership rejection; existing cancellation tests. |
| 7 | Admit total encoded sidecar size, including pending and agent-written turns. | Small exact cap, JSON expansion, both ABIs, fake-provider writeback, reopen. |
| 8 | Bound f64 range sizing, reserve fallibly, and check progress on each step. | Initial/later non-progress, NaN/infinity, just-over-limit rejection, descending/empty ranges. |
| 9 | Cap bytevector construction at 64 MiB and reserve fallibly. | Just-over-limit and unrepresentable-size rejection without allocation. |
| 10 | Keep backend startup/runtime errors and retire failed watcher handles on event polling. | Fake construction and registration failures through the public API; existing lifecycle tests. |
| 11 | Require configured side-set bits for an explicit operand. | Missing/zero configuration errors; existing valid encodings. |
| 12 | Validate WAIT polarity, source-specific index bounds, and IRQ-only relative mode. | Builders and direct maps, invalid combinations, existing golden encodings. |
| 13 | Carry actual Axum upgrade validity to responders; reject before calling WS handlers. | GET/HEAD requests, handler side-effect counter, both ABIs, valid-upgrade control. |
| 14 | Decode path segments after splitting and validate decoded static paths. | Space/Unicode filenames, literal plus, encoded segment boundaries, invalid encodings. |
| 15 | Park on bind readiness through an external wait. | Unsignalled readiness returns a structural suspension; callback tracing and host abort on drop; server async tests. |
| 16 | Terminate Git options before ignore-check filenames. | Ignored and unignored leading-dash names in both ABIs. |
| 17 | Request and parse NUL-delimited Git filename lists. | Literal newline, tab, backslash, and leading-newline names in both ABIs. |
| 18 | Validate command option maps, cwd, and environment fields before spawning. | Field-specific errors from shell/proc/PTY without launching a command. |
| 19 | Accumulate negative scaled integers in the negative domain. | i64 minimum/maximum and adjacent overflow values. |
| 20 | Start deep-merge with an ordered map and validate all top-level inputs. | Empty/singleton/mixed maps and scalar rejection. |
| 21 | Stop each diff hunk after its declared old/new line counts. | Multiple file sections, exact hunk bodies, statistics, header-like content. |
| 22 | Share DOM text traversal that skips script/style/template subtrees. | Both extraction functions, adjacent text, directly selected excluded elements. |
| 23 | Reject negative key timeouts before any stdin read. | Both ABIs and zero-timeout parsing. |

Focused tests were written before fixes where bounded reproduction was safe.
The allocation/non-progress and negative-timeout cases ran only after guards
were installed. The bind regression checks suspension directly without using
an unbounded pre-fix wait. Review also added fallible reservation when a float
range outgrows its initial rounded estimate.

### Final verification

- `jake test` passed at default concurrency: **7,968 passed, 57 skipped**.
  Nextest test time was 97.620 seconds; total recipe time was 280.98 seconds,
  including compilation and test discovery.
- `cargo clippy --workspace -- -D warnings` passed (the CI lint scope).
- `cargo clippy --all-targets -- -D warnings` passed for the default workspace
  members, including all changed Rust modules and their test targets.
- An additional `cargo clippy --workspace --all-targets -- -D warnings` check
  found the existing `items_after_test_module` warning in
  `crates/sema-wasm/src/output.rs:170`. The file is unchanged, and the same
  item ordering is present in HEAD. This unrelated warning was not changed.
- `cargo fmt --all -- --check`, `git diff --check`, and both unified-runtime
  source-policy scripts passed. The hook allowlist count increased only for
  the four new test-only install/clear calls used by byte capture regressions.
- `cargo run --bin sema-docs -- gen --strict` generated all 1,029 entries.
  The generated index includes the updated contracts and synchronizes the
  already-tracked `string/to-number` radix documentation.
- These checks ran against the combined working tree. Concurrent LSP/DAP,
  Git cancellation, record-type, and other test-harness changes were preserved
  and are not included in this pass's commits.
- The diff-based self-review used the `code-review` skill; Rust implementation
  used `idiomatic-rust`. The review checked callback tracing, checkout identity,
  rollback paths, allocation admission, raw-output capture, and route decoding.
  It added checked reservation for float-range growth and corrected stale
  async-server documentation before completion.

### Implementation commits

- `b311d161` — numeric ranges, scaled integers, PIO validation, ordering, and map keys.
- `35b7bbf4` — Unicode/ANSI text, HTML extraction, and diff semantics.
- `2c9961c8` — KV rollback, SQLite checkout identity, and memory persistence limits.
- `55a1e5fa` — byte streams, process options, watcher errors, Git filenames, and key timeouts.
- `ffdd5bcf` — WebSocket upgrade checks, decoded route paths, and cooperative binding.

The prior three stdlib fixes are included in the related implementation commits.
The final documentation commit records this report and regenerates the builtin
index. Nothing was pushed.

## Coverage

| Reviewer | Assigned modules |
| --- | --- |
| Collections | `list`, `map`, `mutable`, `predicates` |
| Numeric | `arithmetic`, `math`, `comparison`, `bitwise`, `typed_array`, `bytevector` |
| Text | `string`, `text`, `regex_ops`, `datetime` |
| Formats | `json`, `toml_ops`, `csv_ops`, `markup`, `archive`, `diff`, `pdf` |
| Files and streams | `io`, `stream`, `fs_watch` |
| Processes | `proc`, `git`, `system`, `pty`, `serial`, `terminal` |
| Async | `async_ops`, `event`, `pio`, `runtime_offload` |
| Storage | `sqlite`, `kv`, `memory`, `secret`, `crypto` |
| Network | `http`, `ws`, `server` |
| Workflow and metadata | `workflow`, `workflow_mcp`, `workflow_check`, `context`, `reflect`, `meta`, `otel` |

## Findings

All source locations below are under `crates/sema-stdlib/src/`.

### 1. High — Unicode case conversion can crash `text/excerpt`

**Location:** `text.rs:222`, `text.rs:228`. **Evidence:** local execution.

The match position comes from lowercased text but is used as a byte offset into
the original text. Case conversion can change byte length. The short input
`(text/excerpt "İéz" "é" {:radius 0})` exits with a Rust character-boundary
panic, rather than returning the matching excerpt. Map match boundaries back
to original text before slicing; test case mappings that expand and shrink.

### 2. High — `sort-by` misorders mixed numeric keys

**Location:** `list.rs:935`. **Evidence:** local execution.

`(sort-by (fn (x) x) (list 2.0 10))` returns `(10 2.0)`, while ordinary `sort`
returns `(2.0 10)`. `Value::cmp` orders these types by rank rather than numeric
value. Compare numeric keys through the numeric tower and define the same
NaN/complex policy as ordinary sorting. Test mixed integer, float, and rational
keys, including a suspending key function.

### 3. High — `diff/apply` loses EOF-newline changes

**Location:** `diff.rs:380`, `diff.rs:504`. **Evidence:** local execution.

`(diff/apply "a" (diff/unified "a" "a\n"))` returns `"a"`. The reverse
conversion also retains the old newline. The output reuses the original
`had_trailing_newline` flag instead of applying patch newline metadata.
Track the target EOF-newline state from hunk lines and test both directions,
including empty input/output and edits to the last line.

### 4. High — Standard byte streams silently replace bytes

**Location:** `stream.rs:2787`, `stream.rs:2820`. **Evidence:** local execution.

Writing byte `FF` to `*stdout*` emits `EF BF BD` but reports one byte written.
Both standard output streams call `String::from_utf8_lossy` on bytevectors.
Provide byte-preserving output, including capture behavior, or explicitly
reject bytes that the output interface cannot represent. Test raw stdout and
stderr bytes separately from the CLI's printed return value.

### 5. High — A failed `kv/set` changes the live store

**Location:** `kv.rs:440`, `kv.rs:592`. **Evidence:** static source trace.

Mutation precedes `flush_store`. A whole-store size rejection or write error
leaves the modified store installed; the async checkout also reinstalls its
resource on operation errors. A subsequent read can therefore observe a value
whose write reported failure, and a later successful write can persist it.
Restore the previous entry on flush failure. Test failure after admission,
using a small test size cap, then check live contents and reopened contents.

### 6. High — Reopening a database can race its previous checkout

**Location:** `sqlite.rs:291`, `sqlite.rs:449`. **Evidence:** static source trace.

`finish_open` unconditionally replaces a named connection and interrupt handle.
An older in-flight checkout subsequently reinstalls its old connection under
that same name. A completed reopen can thus be undone, and an old cancellation
can find the replacement's interrupt handle. Coordinate open with the handle's
gate, reject a busy reopen, or use generations. Add a deterministic test that
holds an old checkout while opening the same name.

### 7. High — Successful memory appends can prevent reopening

**Location:** `memory.rs:740`, `memory.rs:392`, `memory.rs:550`.
**Evidence:** static source trace.

`memory/append` checks individual turn size but not total encoded sidecar size.
Repeated successful appends can exceed the file cap enforced by `memory/open`,
making the persisted transcript fail to reopen. JSON encoding can also expand
the stored byte count. Check cumulative encoded size before accepting a turn
and before writing, or define a rollover policy. Test with a small cap and
verify that earlier successful turns remain reopenable after rejection.

### 8. High — Floating-point ranges can stop making progress

**Location:** `typed_array.rs:338`. **Evidence:** static source trace.

`f64-array/range` rejects zero step but does not check that adding a finite
nonzero step changes the current value. Below floating-point precision, the
loop condition can stay true without progress. Non-finite extent calculations
also reach the capacity cast. Validate finite/bounded sizing and check progress
on each step. Add bounded regression tests that expect a controlled error;
do not test this by allowing an unbounded allocation.

### 9. Medium — `make-bytevector` lacks allocation-size admission

**Location:** `bytevector.rs:92`, `bytevector.rs:111`.
**Evidence:** static source trace.

Any nonnegative `i64` size reaches `vec![fill; size as usize]`. A size outside
allocation limits produces a Rust allocation/capacity failure rather than a
Sema error. Check representability and a defined maximum before allocating,
consistent with other bulk constructors. Test a rejected value just beyond
the limit without attempting that allocation.

### 10. High — Watcher initialization errors are discarded

**Location:** `fs_watch.rs:278`, `fs_watch.rs:285`, `fs_watch.rs:381`.
**Evidence:** static source trace.

The worker silently returns if watcher creation or registration fails, but
`fs/watch` publishes a handle. Its event queue remains empty without exposing
the setup error, and the handle still occupies a registry slot. Return setup
status through a readiness handshake or retain a terminal error on the handle.
Test a fake backend registration failure through the public API.

### 11. High — Side-set without configured bits changes a PIO opcode

**Location:** `pio.rs:321`. **Evidence:** local assembly; no hardware execution.

A JMP assembled normally has instruction bytes `00 00`; applying `pio/side 1`
without side-set configuration produces `00 20`. The unchecked value extends
beyond the delay/side-set field into the opcode. Reject a side-set operand when
no side-set data bits are configured. Test absent and explicitly zero bit
counts, alongside valid configured side-set instructions.

### 12. High — PIO WAIT accepts incompatible source options

**Location:** `pio.rs:215`, `pio.rs:543`.
**Evidence:** local assembly; no hardware execution.

`:rel` is accepted for GPIO/PIN even though its relative meaning applies to IRQ.
For GPIO index 2, it changes the encoded operand from `82` to `92`, selecting
a different GPIO index. IRQ indices are also accepted up to 31 although the
sibling IRQ builder limits flags to 0–7. Validate source-specific options and
ranges in assembly as well as builders. Add invalid-source tests and valid
relative-IRQ golden encodings.

### 13. High — Ordinary HTTP requests can start WebSocket handlers

**Location:** `server.rs:887`, `server.rs:1871`, `server.rs:1366`.
**Evidence:** static handler trace; pure routing also returns a WS marker for HEAD.

The router accepts GET/HEAD for `:ws` without checking upgrade validity.
The runtime starts the handler after publishing its WebSocket response;
the HTTP layer only then checks whether an upgrade exists and returns 400.
Handler side effects can therefore run for an ordinary HTTP request. Carry
upgrade validity into dispatch and reject before invoking the handler. Test
plain GET/HEAD with a side-effect counter and a valid upgrade as control.

### 14. Medium — Encoded path segments remain encoded during routing

**Location:** `server.rs:1266`, `server.rs:895`.
**Evidence:** local pure routing; static-file effect traced in source.

The parameter in `/search/hello%20world` is returned as `hello%20world`.
Static-file lookup likewise uses the encoded path, so a URL for a filename
containing a space looks up the wrong filename. Decode path segments without
changing their structural boundaries, then validate the decoded path before
file access. Test space-containing parameters and filenames, while preserving
literal plus signs and path-boundary behavior.

### 15. Medium — Server startup blocks the cooperative runtime

**Location:** `server.rs:2941`, `server.rs:2324`.
**Evidence:** static source trace; startup delay was not measured.

The runtime ABI calls `http_serve_setup`, which waits on synchronous
`ready_rx.recv()` for binding to finish. Sibling tasks and cancellation cannot
advance on that VM thread during the wait. Return a cooperative external wait
for bind readiness. Use a delayed fake readiness source to test sibling progress.

### 16. Medium — Git ignore checks misinterpret leading-dash paths

**Location:** `git.rs:607`, `git.rs:794`. **Evidence:** local execution.

`git/ignore-matches?` passes a pathname to `git check-ignore` without an option
terminator. A leading-dash pathname is interpreted as an option and returns
exit 129 instead of a boolean. Add `--` before the pathname in both paths.
Test ignored and unignored leading-dash filenames in synchronous and runtime
dispatch.

### 17. Medium — Git filename lists return quoted representations

**Location:** `git.rs:180`, `git.rs:735`, `git.rs:760`.
**Evidence:** static command/decoder trace; no filename fixture created.

`git/diff-files` and `git/recent-files` request newline-delimited name output
and split it with `.lines()`. Git quotes names containing newline, tab, or
backslash, so these results are not literal filenames. Request NUL-delimited
output and decode it as such, accounting for log record separators. Test
round-tripping unusual but valid filenames in both dispatch paths.

### 18. Medium — Invalid process options silently use inherited settings

**Location:** `system.rs:611`, `proc.rs:313`, `pty.rs:236`.
**Evidence:** reviewer local execution and parent source review.

Invalid `:cwd`/`:env` fields are dropped rather than rejected. For example,
`proc/spawn` with `{:cwd 1}` launches in the inherited directory. A non-map
options argument is also ignored by some callers. Return a `Result` from
option parsing and validate supplied fields and environment entries. Test
rejection before any process starts.

### 19. Medium — Scaled integer parsing excludes `i64::MIN`

**Location:** `bytevector.rs:54`, `bytevector.rs:81`.
**Evidence:** local execution.

`(bytes/parse-int10 (string->utf8 "-922337203685477580.8"))` reports overflow
although the scaled result is exactly `i64::MIN`. The parser accumulates the
positive magnitude in an `i64` before negation. Accumulate negative inputs in
the negative domain or use a wider magnitude. Test the minimum and both
adjacent out-of-range boundaries.

### 20. Medium — Singleton `deep-merge` violates its ordered-map contract

**Location:** `map.rs:1034`, `map.rs:1077`. **Evidence:** local execution.

`(type (deep-merge (hashmap/new :a 1)))` is `:hashmap`, contrary to the documented
always-ordered result. The first argument is returned unchanged when there
are no more inputs. Non-map top-level arguments also pass through, such as
`(deep-merge 1 2)` returning `2`. Validate inputs and normalize the initial
accumulator. Test empty, singleton, mixed-map, and invalid inputs.

### 21. Medium — Multi-file diff headers become hunk contents

**Location:** `diff.rs:325`, `diff.rs:565`. **Evidence:** local execution.

For two ordinary `---`/`+++` file sections with one replacement each and no Git
preamble, `diff/stat` reports three additions and three removals instead of
two each. The second file's headers are accepted as the first hunk's body.
Track consumed old/new line counts and end a completed hunk before accepting
new headers. Test both statistics and exact hunk bodies, including content
lines that genuinely begin with repeated `+` or `-` characters.

### 22. Medium — HTML visible-text extraction includes script/style source

**Location:** `markup.rs:112`, `markup.rs:124`. **Evidence:** local execution.

`html/text` returns script and style source alongside paragraph text despite
its visible-text contract. It collects every descendant text node without
excluding non-rendered elements. Use a shared filtered DOM traversal for
`html/text` and `html/select-text`. Test script/style descendants and adjacent
visible text; document the limits of extraction without a CSS renderer.

### 23. Medium — Negative key-input timeouts become huge positive waits

**Location:** `io.rs:2653`, `io.rs:2714`. **Evidence:** static source trace.

Both ABIs cast the signed timeout directly to `u64`. A negative value becomes
a very large wait when stdin remains open, rather than an input error.
Validate with a checked conversion before reading stdin. Test rejection in
both dispatch paths without relying on a real keyboard or elapsed-time wait.

## Excluded and unresolved candidates

- **MCP resolver omissions:** withdrawn. Production resolver paths return one
  encoded result per declaration. The reported omission required a host hook
  to violate its contract; no normal Sema input path was found.
- **`read/string` suffix forms:** withdrawn. It follows the legacy first-form
  reader contract. Clarify its documentation rather than changing semantics.
- **Unicode chunk size:** deferred. `text/chunk` mixes byte and character
  counts, but its documentation does not specify the unit. The existing ASCII
  test cannot establish a byte contract. Choose and document a unit first.
- **NaN in set-like list operations:** deferred. `Ord`-based helpers differ
  from equality-based helpers. Decide the intended non-reflexive-value policy
  before treating all such differences as separate bugs.

The text reviewer also found that the first ANSI patch retained unrelated
post-cutoff terminal controls. That issue was corrected in this patch and
independently re-reviewed; it is not an outstanding finding above.

## Summary

| Severity | Count | Evidence |
| --- | --- | --- |
| High | 12 | 6 local execution, 6 static traces |
| Medium | 11 | 7 local execution, 4 static traces |
| Total | 23 | 13 local execution, 10 static traces |

The table records the original review severities. All 23 retained findings have
fixes and regression coverage; the excluded candidates remain outside this pass.
**Self-review verdict: approve** for the 23 fixes and their regression tests.
The unrelated WASM test-target lint and excluded candidates are noted above;
this is not a claim that the whole repository is free of defects.
