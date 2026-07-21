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
  SET_CALL_OWNED_CALLBACK

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
# sanctioned shape and must PASS.
expect_success \
  "$fixtures/negated-runtime-io-block-on.rs" \
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

echo "ok: unified-runtime source-policy fixtures"
