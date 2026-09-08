# Build profile measurements — 2026-09-08

Use `jake release-fast` for local work that needs an optimized binary. On this
machine it cut a fresh optimized build from 258.5 s to 102.6 s, and a CLI
comment-edit rebuild from 167.3 s to 9.5 s. Runtime was 5.2% slower by the
geometric mean across 12 benchmarks. The largest measured slowdown was 12.0%.

The priority for this pass was build time, then runtime performance, then size.

## Configuration

`cargo build --profile release-fast` produces `target/release-fast/sema`.
The profile inherits `release`, including `opt-level = 3`, `panic = "abort"`,
and `strip = "debuginfo"`, with these overrides:

| Setting | release | release-fast |
| --- | --- | --- |
| LTO | fat | false |
| Codegen units | 1 | 16 |
| Incremental | false | true |

In Cargo, `lto = false` permits local ThinLTO within each crate; it disables
cross-crate LTO. More codegen units permit parallel code generation.
See the [Cargo profile reference](https://doc.rust-lang.org/cargo/reference/profiles.html).

Use `jake release`, `cargo build --release`, and the existing benchmark recipes
for release performance measurements. The `release-fast` profile is an explicit
local-build option. No nightly compiler or additional linker is needed.

## Build measurements

Source revision: `482720fa`, plus the profile and recipe changes in this report.
Machine: Apple M2 Max, 12 CPUs, 32 GiB RAM, macOS 15.6.
Compiler: Rust 1.98.0 (`88d9e12ae`, 2026-08-18), LLVM 22.1.8.

Each build timing is one run. Profile output directories were fresh for the
initial builds; dependency downloads were already available. Some other local
work was active during the initial builds. These results establish the direction
and approximate scale of the change, not a confidence interval.

| Operation | release | release-fast | Speedup |
| --- | --- | --- | --- |
| Fresh optimized build | 258.50 s | 102.57 s | 2.52× |
| CLI comment-edit rebuild | 167.33 s | 9.53 s | 17.56× |
| Binary size, decimal MB | 32.99 | 42.03 | — |
| Fresh build CPU user time | 556.38 s | 679.73 s | — |

Both rebuilds followed the same added comment at the start of
`crates/sema/src/main.rs`, after the initial build of each profile. The comment
was removed after measurement. This measures a small, nonsemantic edit; changes
to function bodies or dependency APIs can invalidate more incremental work.

The faster cold build used more CPU time. Parallel code generation reduced wall
time; it did not reduce total compiler work. The binary grew by 27.4%.

Cargo's timing report put the release CLI compiler step at 178.0 s. A macOS
`sample` capture found the active compiler in
`optimize_and_codegen_fat_lto` and LLVM machine-code generation.

The native dev baseline was 54.43 s from a fresh target. Building all default
members' tests after that took 44.82 s. Both used the existing incremental dev
profile with line-table debug information.

## Runtime measurements

Hyperfine ran five measurements and one warmup for each binary on each benchmark.
Every measured command exited successfully. Times below are means in milliseconds;
the last column is the increase in elapsed time for `release-fast`.
The geometric mean increase was 5.15%. The small Mandelbrot sample included a
reported statistical outlier. Treat small differences as indicative.

| Benchmark | release, ms | release-fast, ms | Time increase |
| --- | --- | --- | --- |
| bench-features | 1787.3 | 1893.3 | +5.9% |
| closure-storm | 725.7 | 740.6 | +2.1% |
| deriv | 706.7 | 773.3 | +9.4% |
| hashmap-bench | 3406.5 | 3613.9 | +6.1% |
| higher-order-fold | 398.0 | 403.5 | +1.4% |
| mandelbrot | 167.2 | 181.5 | +8.5% |
| nqueens | 1619.2 | 1813.2 | +12.0% |
| recursive-closure-churn | 105.3 | 105.5 | +0.2% |
| string-pipeline | 569.1 | 624.6 | +9.8% |
| tak | 1310.2 | 1318.9 | +0.7% |
| throw-catch | 345.5 | 364.0 | +5.3% |
| upvalue-counter | 416.4 | 421.8 | +1.3% |

[Recorded samples and configuration](build-profiles-2026-09-08.json) include
individual elapsed times, standard deviations, and exit codes.

## Linker experiment

The installed Homebrew LLD 22.1.4 did not improve the measured dev CLI link.
The compiler's `run_linker` timing was 0.764 s with Apple ld 1230.1 and 1.491 s
with LLD. Each is one sample. LLD also emitted deployment-target warnings for
native dependencies. The existing linker remains the appropriate choice for
this measured path.

This used `RUSTC_BOOTSTRAP=sema cargo rustc -p sema-lang --bin sema -- -Z time-passes`,
then the same command with
`-C link-arg=-fuse-ld=/opt/homebrew/bin/ld64.lld`. These were temporary diagnostic
commands. No bootstrap variable or linker override was added to configuration.
The package-specific command changes feature selection, so its overall Cargo
time is not compared with the default-member builds above.

## Reproduction

Create an isolated worktree with `jake wt-new` from the workspace root. In that
worktree, run these commands once for fresh builds:

```bash
/usr/bin/time -p cargo build --locked --release --timings
/usr/bin/time -p cargo build --locked --profile release-fast --timings
```

For the rebuild comparison, add a comment to `crates/sema/src/main.rs`, run both
commands again, then remove the comment.

For one runtime comparison:

```bash
hyperfine --runs 5 --warmup 1 \
  'target/release/sema --no-llm examples/benchmarks/nqueens.sema' \
  'target/release-fast/sema --no-llm examples/benchmarks/nqueens.sema'
```

Copy any results to keep before removing the worktree with `jake wt-rm`.
See [the July investigation](build-time-report.md) for the existing dev/test
build policy and earlier dependency and test-consolidation changes.

## Verification

- `jake lint` passed (format check and workspace Clippy with warnings denied).
- Fresh builds of both optimized profiles passed. All 120 measured runtime
  executions exited successfully.
- `cargo nextest run --workspace --locked --no-fail-fast` ran 7,871 tests:
  7,869 passed, two failed, and 57 were skipped.
- Both failing tests reproduced in the untouched main checkout at `482720fa`,
  before applying the profile changes:
  `map_callback_async_test::hashmap_nan_entries_survive_every_async_map_traversal`
  expects NaN map keys to work, and `test_define_record_type_mutator_ignored`
  expects a three-item record field declaration to work. The implementation
  rejects both inputs.
- The additional `cargo clippy --workspace --all-targets -- -D warnings` check
  found the existing `clippy::items_after_test_module` error in
  `crates/sema-wasm/src/output.rs:145`. That file is unchanged by this work.
- Diff review found no correctness issues in the profile or recipe changes.
