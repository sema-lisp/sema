#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scanner="$repo_root/scripts/check-unified-runtime-legacy.sh"
fixtures="$repo_root/scripts/fixtures/unified-runtime-source-policy"

expect_failure() {
  local fixture="$1"
  local allowlist="$2"
  shift 2
  local output
  if output="$($scanner --check-policy-path "$fixture" "$allowlist" 2>&1)"; then
    echo "source-policy fixture unexpectedly passed: $fixture" >&2
    exit 1
  fi
  for expected in "$@"; do
    if [[ "$output" != *"$expected"* ]]; then
      echo "source-policy fixture did not report '$expected': $fixture" >&2
      echo "$output" >&2
      exit 1
    fi
  done
}

expect_success() {
  local fixture="$1"
  local allowlist="$2"
  local output
  if ! output="$($scanner --check-policy-path "$fixture" "$allowlist" 2>&1)"; then
    echo "source-policy fixture unexpectedly failed: $fixture" >&2
    echo "$output" >&2
    exit 1
  fi
}

expect_failure \
  "$fixtures/deleted-bridges.rs" \
  "$fixtures/empty-allowlist.tsv" \
  CURRENT_VM \
  CurrentVmGuard \
  try_run_on_current_vm \
  try_run_on_current_vm_args \
  run_nested_closure_args \
  current_vm_globals \
  suspend_runtime_quantum \
  QuantumSuspendGuard \
  snapshot_escaping_closure \
  snapshot_escaping_value \
  snapshot_native_escaping_args_for_current_vm

expect_failure \
  "$fixtures/unallowlisted-host-adapters.rs" \
  "$fixtures/empty-allowlist.tsv" \
  EVAL_CALLBACK \
  EVAL_CALLBACK_FN \
  EVAL_FN \
  SET_EVAL_CALLBACK \
  CALL_CALLBACK \
  CALL_CALLBACK_OWNED \
  WITH_STDLIB_CTX \
  SET_CALL_CALLBACK \
  SET_CALL_OWNED_CALLBACK \
  HOST_OUTPUT_HOOK \
  HOST_SANDBOX

# This fixture is deliberately allowlisted by exact token counts. It must still
# fail because it places synchronous callback re-entry inside an active runtime
# branch explicitly.
expect_failure \
  "$fixtures/active-runtime-callback.rs" \
  "$fixtures/active-runtime-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  CALL_CALLBACK

expect_failure \
  "$fixtures/active-runtime-multiline-callback.rs" \
  "$fixtures/active-runtime-multiline-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  CALL_CALLBACK

expect_failure \
  "$fixtures/active-context-callback.rs" \
  "$fixtures/active-context-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  CALL_CALLBACK

expect_failure \
  "$fixtures/active-runtime-eval-callback.rs" \
  "$fixtures/active-runtime-eval-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  EVAL_CALLBACK

expect_failure \
  "$fixtures/active-context-eval-callback.rs" \
  "$fixtures/active-context-eval-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  EVAL_CALLBACK

expect_failure \
  "$fixtures/active-runtime-nested-callback.rs" \
  "$fixtures/active-runtime-nested-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  CALL_CALLBACK

# A workflow-scope TLS read allowlisted by exact count must STILL fail when it sits
# inside an active runtime branch: the runtime path must reach the live run through the
# owning task context, never the `WORKFLOW` thread-local.
expect_failure \
  "$fixtures/active-runtime-workflow-tls.rs" \
  "$fixtures/active-runtime-workflow-tls-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  WORKFLOW_TLS

expect_failure \
  "$fixtures/active-runtime-compound-negation.rs" \
  "$fixtures/active-runtime-compound-negation-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  CALL_CALLBACK

expect_failure \
  "$fixtures/active-runtime-rust-lifetimes.rs" \
  "$fixtures/active-runtime-rust-lifetimes-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  CALL_CALLBACK

# A quantum-reachable `io_block_on` must fail: runtime code parks on an External
# wait; `io_block_on` is a host-only adapter, legal only on the
# `!in_runtime_quantum()` arm (or inside an `io_spawn_blocking` worker), never
# textually inside an active-runtime branch.
expect_failure \
  "$fixtures/active-runtime-io-block-on.rs" \
  "$fixtures/empty-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  IO_BLOCK_ON

# The same adapter on the negated (`!in_runtime_quantum()`) host arm is the
# sanctioned shape and must PASS — it is a counted host adapter, so it carries its
# own exact-count IO_BLOCK_ON allowlist (as every retained provider/host site does).
expect_success \
  "$fixtures/negated-runtime-io-block-on.rs" \
  "$fixtures/negated-runtime-io-block-on-allowlist.tsv"

# R08C: a raw blocking `std::io::stdin()` read inside an active-runtime branch
# must fail — runtime code reads stdin through the coordinated owner, which parks
# structurally. The wasm host-adapter fallbacks (`io.rs` `read-line`/`read-stdin`)
# stay on the `!in_runtime_quantum()` arm and are unaffected.
expect_failure \
  "$fixtures/active-runtime-stdin-read.rs" \
  "$fixtures/empty-allowlist.tsv" \
  "active-runtime synchronous callback re-entry" \
  RAW_STDIN_READ

# The same blocking stdin read on the negated (host) arm is the sanctioned shape
# and must PASS.
expect_success \
  "$fixtures/negated-runtime-stdin-read.rs" \
  "$fixtures/empty-allowlist.tsv"

expect_success \
  "$fixtures/negated-runtime-host-adapter.rs" \
  "$fixtures/negated-runtime-host-allowlist.tsv"

expect_success \
  "$fixtures/permitted-host-adapter.rs" \
  "$fixtures/permitted-host-adapter-allowlist.tsv"

expect_failure \
  "$fixtures/permitted-host-adapter.rs" \
  "$fixtures/stale-count-allowlist.tsv" \
  "restricted host-adapter count changed" \
  "expected 2, found 1"

# Deleted/restricted names in line comments are documentation, not live code.
expect_success \
  "$fixtures/comments-only.rs" \
  "$fixtures/empty-allowlist.tsv"

# ── A2 workflow-journal filesystem policy ───────────────────────────────────
wf_journal_allowlist="$repo_root/scripts/workflow-journal-fs-allowlist.tsv"

expect_journal_success() {
  local fixture="$1" allowlist="$2" output
  if ! output="$($scanner --check-workflow-journal "$fixture" "$allowlist" 2>&1)"; then
    echo "workflow-journal fixture unexpectedly failed: $fixture" >&2
    echo "$output" >&2
    exit 1
  fi
}

expect_journal_failure() {
  local fixture="$1" allowlist="$2"
  shift 2
  local output
  if output="$($scanner --check-workflow-journal "$fixture" "$allowlist" 2>&1)"; then
    echo "workflow-journal fixture unexpectedly passed: $fixture" >&2
    exit 1
  fi
  for expected in "$@"; do
    if [[ "$output" != *"$expected"* ]]; then
      echo "workflow-journal fixture did not report '$expected': $fixture" >&2
      echo "$output" >&2
      exit 1
    fi
  done
}

# The sanctioned shape (create_new claim, parent+memo create_dir_all only) PASSES.
expect_journal_success \
  "$fixtures/workflow-journal-clean.rs" \
  "$wf_journal_allowlist"

# Reintroducing the exists-probe segment claim FAILS, even with the allowlisted
# create_dir_all count intact.
expect_journal_failure \
  "$fixtures/workflow-journal-exists-probe.rs" \
  "$wf_journal_allowlist" \
  WORKFLOW_SEGMENT_EXISTS_PROBE

# ── A3 workflow-writer filesystem policy ────────────────────────────────────
wf_writer_zero_allowlist="$fixtures/workflow-writer-zero-allowlist.tsv"

expect_writer_success() {
  local fixture="$1" allowlist="$2" output
  if ! output="$($scanner --check-workflow-writer "$fixture" "$allowlist" 2>&1)"; then
    echo "workflow-writer fixture unexpectedly failed: $fixture" >&2
    echo "$output" >&2
    exit 1
  fi
}

expect_writer_failure() {
  local fixture="$1" allowlist="$2"
  shift 2
  local output
  if output="$($scanner --check-workflow-writer "$fixture" "$allowlist" 2>&1)"; then
    echo "workflow-writer fixture unexpectedly passed: $fixture" >&2
    exit 1
  fi
  for expected in "$@"; do
    if [[ "$output" != *"$expected"* ]]; then
      echo "workflow-writer fixture did not report '$expected': $fixture" >&2
      echo "$output" >&2
      exit 1
    fi
  done
}

# The write-free A3 journal shape (writes moved to the writer thread) PASSES the zero
# allowlist.
expect_writer_success \
  "$fixtures/workflow-journal-clean.rs" \
  "$wf_writer_zero_allowlist"

# Reintroducing a synchronous `Journal::write` fs call (write_all + fs::write on the VM
# thread) FAILS — journal.rs must stay write-free.
expect_writer_failure \
  "$fixtures/workflow-journal-sync-write.rs" \
  "$wf_writer_zero_allowlist" \
  WORKFLOW_WRITE_ALL \
  WORKFLOW_FS_WRITE

# ── B6 spinner lifecycle policy (R19) ───────────────────────────────────────
spinner_park_allowlist="$repo_root/scripts/spinner-park-allowlist.tsv"

expect_spinner_success() {
  local fixture="$1" output
  if ! output="$($scanner --check-spinner-park "$fixture" "$spinner_park_allowlist" 2>&1)"; then
    echo "spinner-park fixture unexpectedly failed: $fixture" >&2
    echo "$output" >&2
    exit 1
  fi
}

expect_spinner_failure() {
  local fixture="$1"
  shift
  local output
  if output="$($scanner --check-spinner-park "$fixture" "$spinner_park_allowlist" 2>&1)"; then
    echo "spinner-park fixture unexpectedly passed: $fixture" >&2
    exit 1
  fi
  for expected in "$@"; do
    if [[ "$output" != *"$expected"* ]]; then
      echo "spinner-park fixture did not report '$expected': $fixture" >&2
      echo "$output" >&2
      exit 1
    fi
  done
}

# The sanctioned condvar-parking render loop (no thread::sleep) PASSES.
expect_spinner_success "$fixtures/spinner-condvar-park.rs"

# A reintroduced bare `thread::sleep` frame loop FAILS the spinner-park policy.
expect_spinner_failure \
  "$fixtures/spinner-frame-sleep.rs" \
  SPINNER_FRAME_SLEEP

# ── C2 OTel file-exporter writer filesystem policy (C06) ────────────────────
otel_writer_zero_allowlist="$fixtures/otel-file-writer-zero-allowlist.tsv"

expect_otel_writer_success() {
  local fixture="$1" allowlist="$2" output
  if ! output="$($scanner --check-otel-file-writer "$fixture" "$allowlist" 2>&1)"; then
    echo "sema-otel file-writer fixture unexpectedly failed: $fixture" >&2
    echo "$output" >&2
    exit 1
  fi
}

expect_otel_writer_failure() {
  local fixture="$1" allowlist="$2"
  shift 2
  local output
  if output="$($scanner --check-otel-file-writer "$fixture" "$allowlist" 2>&1)"; then
    echo "sema-otel file-writer fixture unexpectedly passed: $fixture" >&2
    exit 1
  fi
  for expected in "$@"; do
    if [[ "$output" != *"$expected"* ]]; then
      echo "sema-otel file-writer fixture did not report '$expected': $fixture" >&2
      echo "$output" >&2
      exit 1
    fi
  done
}

# The sanctioned C2 shape (render + enqueue, writes on the writer thread) PASSES the zero
# allowlist — a non-writer sema-otel path is write-free.
expect_otel_writer_success \
  "$fixtures/otel-file-writer-clean.rs" \
  "$otel_writer_zero_allowlist"

# Reintroducing a synchronous per-span-end fs write (write_all + fs::write on the VM thread)
# FAILS — non-writer sema-otel paths must stay write-free.
expect_otel_writer_failure \
  "$fixtures/otel-file-writer-sync-write.rs" \
  "$otel_writer_zero_allowlist" \
  SEMA_OTEL_WRITE_ALL \
  SEMA_OTEL_FS_WRITE

echo "ok: unified-runtime source-policy fixtures"
