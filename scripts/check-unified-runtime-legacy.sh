#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ── Fixture pattern (used by --scan-path) ───────────────────────────────────
# Broad "legacy async" markers used only to prove the scanner still detects a
# raw blocking-recv fixture (see the conformance test). NOT the removal gate.
legacy_pattern='IoHandle|IoPoll|YieldReason|SchedulerTarget|SchedulerRunResult|set_yield_signal|take_yield_signal|set_resume_value|take_resume_value|set_(eval|call|call_owned|spawn|cancel|run_scheduler)_callback|eval_callback|call_callback(_owned)?|call_(spawn|cancel|run_scheduler)|run_until_reentrant|tasks\.remove\(|in_async_context|io_block_on|\bblock_on\b|thread::sleep|std::thread::sleep|blocking_recv|recv_timeout|\.recv\(\)|HTTP_AWAIT_MARKER|MAX_REPLAYS|XmlHttpRequest|XMLHttpRequest|Atomics::wait|Atomics\.wait|installAtomicsSleep'

# ── Zero-tolerance removal gate (P5 "the purge") ────────────────────────────
# Identifiers deleted by the legacy-async-scheduler purge. Reintroducing ANY of
# them in shipped code (comment-stripped) fails this gate. These are the exact
# symbols retired with `scheduler.rs`, the cooperative-debug driver, and the
# legacy `async_signal.rs` seams — every async op is now structural.
#
# Deliberately NOT here (KEPT, still live): `YieldReason` (reduced to `Sleep`),
# `set_yield_signal`/`take_yield_signal`/`take_resume_value` (the ctx-less
# `async/sleep` bridge), `VmExecResult::AsyncYield` (carries `Sleep`),
# `execute_debug` (the VM-level debug test driver), `in_runtime_quantum`.
#
# Also deliberately NOT here (P6-3 step 5 — see
# `docs/plans/2026-07-16-wasm-promise-driven-roots.md` §3 and the P6-3 entry
# in `docs/deferred.md`): `HTTP_AWAIT_MARKER`/`is_http_await_marker`/
# `parse_http_marker`/`HTTP_CACHE`/`clear_http_cache`/`perform_fetch_from_marker`
# stay live — narrowed to the wasm DEBUGGER's own `http_needed`/
# `debugPerformFetch` flow (`debugStart` is not promise-driven and has no
# other way to surface a pending fetch to JS); `SLEEP_I32`/
# `worker_atomics_sleep`/`worker_check_interrupt`/`installAtomicsSleep`/
# `set_blocking_sleep_callback`/`set_interrupt_callback`/`check_interrupt`
# stay live too — `crates/sema-eval/src/eval.rs`'s `drive_handle_to_settlement`
# (wasm32 branch) still needs interruptible blocking sleep for every
# still-synchronous wasm entry point (`eval`/`evalGlobal`/`evalVM`, and a
# precompiled bytecode archive entry), which structurally suspend on a bare
# `(async/sleep ...)` exactly like the promise-driven path does (`async/sleep`
# is not dual-ABI-gated). Only `MAX_REPLAYS` (no remaining caller — the three
# replay loops it bounded are gone) and the worker's `legacySab`/control-SAB
# allocation (JS; the browser gate's own step-4 scoping note already flagged
# it as dormant) are unconditionally deleted; those two ARE in the
# zero-tolerance list below.
purged_pattern='LegacyPromise|LegacyChannel|\bIoHandle\b|\bIoPoll\b|SchedulerTarget|SchedulerRunResult|DebugCoopResume|set_debug_coop_resume|take_debug_coop_resume|debug_coop_resume_pending|set_resume_value|\bin_async_context\b|set_async_context|init_scheduler|shutdown_scheduler|reset_scheduler_tasks|scheduler_task_count|run_cooperative|start_cooperative|run_closure_as_inline_task|call_run_scheduler|call_run_scheduler_all_of|call_run_scheduler_any_of|call_run_scheduler_target|call_run_scheduler_timeout|set_run_scheduler_callback|call_spawn_callback|set_spawn_callback|call_cancel_callback|set_cancel_callback|notify_io_complete|\bio_park\b|PromiseSetKind|LegacyRuntimeBridge|with_coop_paused_task_vm|COOP_TASK_STOP|coop_paused_task_id|clear_coop_paused_task_id|surface_coop_task_stop|reconstruct_coop_resume_value|\bexecute_async\b|\brun_async\b|\bMAX_REPLAYS\b|legacySab|new SharedArrayBuffer\('

# Exact-file allowlist (no globs). A purged identifier surviving here is a KNOWN,
# reviewed exception with a written reason. Currently empty — the purge is total.
# Format: one "path-suffix|reason" per line.
purged_allowlist=()

scan_legacy_symbols() {
  cd "$repo_root"
  rg -n --with-filename --no-heading --color never \
    -g '*.rs' \
    -g '*.js' \
    -g '*.ts' \
    -g '!crates/sema/src/web/assets/**' \
    -g '!playground/src/examples.js' \
    "$legacy_pattern" \
    "$@" \
    | LC_ALL=C sort -u
}

require_nonempty_scan() {
  local scan_file="$1"
  if [[ ! -s "$scan_file" ]]; then
    echo "legacy scan returned no matches; scanner coverage is broken" >&2
    exit 2
  fi
}

# Comment-stripped, word-bounded zero-tolerance scan for the purged identifiers
# over shipped source. `//`-to-EOL and `;;`-to-EOL (Sema) comments are removed
# first so stale doc references don't trip the gate — only live code counts.
scan_purged() {
  cd "$repo_root"
  local files
  files=$(rg --files -g '*.rs' -g '*.js' -g '*.ts' \
    -g '!crates/sema/src/web/assets/**' -g '!playground/src/examples.js' \
    crates/*/src playground/src 2>/dev/null || true)
  local hits=""
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    local matched
    matched=$(sed -E 's://.*$::; s:;;.*$::' "$f" \
      | rg -n --no-heading --color never "$purged_pattern" || true)
    if [[ -n "$matched" ]]; then
      while IFS= read -r line; do
        hits+="$f:$line"$'\n'
      done <<< "$matched"
    fi
  done <<< "$files"
  # Apply the exact-file allowlist.
  local filtered=""
  while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    local allowed=0
    for entry in ${purged_allowlist[@]+"${purged_allowlist[@]}"}; do
      local suffix="${entry%%|*}"
      case "$hit" in
        *"$suffix"*) allowed=1; break ;;
      esac
    done
    [[ "$allowed" -eq 0 ]] && filtered+="$hit"$'\n'
  done <<< "$hits"
  printf '%s' "$filtered"
}

case "${1:-}" in
  --scan-path)
    if [[ $# -ne 2 ]]; then
      echo "usage: $0 --scan-path PATH" >&2
      exit 2
    fi
    current="$(mktemp)"
    trap 'rm -f "$current"' EXIT
    scan_legacy_symbols "$2" >"$current"
    require_nonempty_scan "$current"
    cat "$current"
    ;;
  --scan-production)
    cd "$repo_root"
    scan_legacy_symbols crates/*/src playground/src
    ;;
  --scan-purged)
    scan_purged
    ;;
  ""|--check)
    hits="$(scan_purged)"
    if [[ -n "${hits//[$'\n']/}" ]]; then
      echo "PURGED legacy-scheduler symbols reintroduced in shipped code:" >&2
      echo "$hits" >&2
      echo "these identifiers were deleted by the async-runtime purge (P5) and must stay deleted" >&2
      exit 1
    fi
    echo "ok: no purged legacy-scheduler symbols in shipped code"
    ;;
  *)
    echo "usage: $0 [--check|--scan-purged|--scan-path PATH]" >&2
    exit 2
    ;;
esac
