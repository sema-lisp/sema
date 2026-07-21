#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
host_adapter_allowlist="$repo_root/scripts/unified-runtime-host-adapters.tsv"

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
# Deliberately NOT here (KEPT, still live): `execute_debug` (the VM-level debug
# test driver), `in_runtime_quantum`.
#
# `YieldReason`, `set_yield_signal`, `take_yield_signal`, and
# `VmExecResult::AsyncYield` (the last surviving TLS yield-signal transport —
# the ctx-less `async/sleep` value-ABI bridge) WERE here in the "KEPT, still
# live" list; they are now retired and moved into `purged_pattern` below.
# `async/sleep`'s structural Timer ABI (`invoke_runtime`) is always preferred
# when a `TaskContext` is installed, so the legacy value-ABI closure is
# reached only when a caller bypasses `invoke_runtime` — a raw native passed
# directly to a single-ABI HOF (`any`/`every`/…) or to `apply` — where there
# is no way to suspend anyway; it now raises a clear error itself instead of
# setting a TLS signal for the VM to relay. `TaskAction::VmSleep`, the
# runtime's sole consumer of the carried `Sleep(ms)`, is retired alongside it.
#
# P6-3's WASM-specific replay cache, debugger fetch bridge, and Atomics host
# adapter are retired and guarded by browser/artifact tests. The host-neutral
# blocking-sleep and interrupt callback APIs remain for native synchronous
# operations; they are not a browser scheduler compatibility path.
#
# The terminal bridge-removal pass also deleted the raw-pointer CURRENT_VM
# stack, nested/fresh-VM callback drive, runtime-quantum suspension escape hatch,
# and ambient snapshot helpers. Those symbols are part of the same
# zero-tolerance policy even though they postdate the P5 purge. The evaluator
# and callable callbacks remain guarded, exact-allowlisted host adapters.
purged_pattern='LegacyPromise|LegacyChannel|\bIoHandle\b|\bIoPoll\b|SchedulerTarget|SchedulerRunResult|DebugCoopResume|set_debug_coop_resume|take_debug_coop_resume|debug_coop_resume_pending|\bRESUME_VALUE\b|set_resume_value|take_resume_value|\bin_async_context\b|set_async_context|init_scheduler|shutdown_scheduler|reset_scheduler_tasks|scheduler_task_count|run_cooperative|start_cooperative|run_closure_as_inline_task|call_run_scheduler|call_run_scheduler_all_of|call_run_scheduler_any_of|call_run_scheduler_target|call_run_scheduler_timeout|set_run_scheduler_callback|call_spawn_callback|set_spawn_callback|call_cancel_callback|set_cancel_callback|notify_io_complete|\bio_park\b|PromiseSetKind|LegacyRuntimeBridge|with_coop_paused_task_vm|COOP_TASK_STOP|coop_paused_task_id|clear_coop_paused_task_id|surface_coop_task_stop|reconstruct_coop_resume_value|\bexecute_async\b|\brun_async\b|\bMAX_REPLAYS\b|legacySab|new SharedArrayBuffer\(|\bYieldReason\b|set_yield_signal|take_yield_signal|\bAsyncYield\b|\bVmSleep\b|\bWsConnectProbe\b|\bWsRecvProbe\b|\bServerWsRecvProbe\b|\bRUNTIME_POLL_COMPLETION_KIND\b|\bRuntimePollDecoder\b|\bCURRENT_VM\b|\bCurrentVmGuard\b|\btry_run_on_current_vm\b|\btry_run_on_current_vm_args\b|\brun_nested_closure_args\b|\bcurrent_vm_globals\b|\bsuspend_runtime_quantum\b|\bQuantumSuspendGuard\b|\bsnapshot_escaping_closure\b|\bsnapshot_escaping_value\b|\bsnapshot_native_escaping_args_for_current_vm\b'

# Synchronous evaluator callbacks remain only as reviewed host-compatibility
# adapters. Each entry in `unified-runtime-host-adapters.tsv` names one token,
# one exact repository path, its exact comment-stripped hit count, and a written
# reason. A new file, a changed count, or a missing/stale allowlist row fails.
# The order keeps longer names ahead of their prefixes for readable diagnostics;
# word boundaries prevent setter names from also matching invocation names.
restricted_tokens=(
  'SET_EVAL_CALLBACK|\bset_eval_callback[[:space:]]*\('
  'EVAL_CALLBACK|\beval_callback[[:space:]]*\('
  'EVAL_CALLBACK_FN|\bEvalCallbackFn\b'
  'EVAL_FN|\beval_fn\b'
  'CALL_CALLBACK_OWNED|\bcall_callback_owned[[:space:]]*\('
  'CALL_CALLBACK|\bcall_callback[[:space:]]*\('
  'WITH_STDLIB_CTX|\bwith_stdlib_ctx([[:space:]]*<|[[:space:]]*\()'
  'SET_CALL_OWNED_CALLBACK|\bset_call_owned_callback[[:space:]]*\('
  'SET_CALL_CALLBACK|\bset_call_callback[[:space:]]*\('
  'WORKFLOW_TLS|\bWORKFLOW\.with\b'
)

# Exact-file allowlist (no globs). A purged identifier surviving here is a KNOWN,
# reviewed exception with a written reason. Currently empty — the purge is total.
# Format: one "path-suffix|reason" per line.
purged_allowlist=()

source_files() {
  local root
  for root in "$@"; do
    if [[ -f "$root" ]]; then
      case "$root" in
        *.rs|*.js|*.ts) printf '%s\n' "$root" ;;
      esac
    elif [[ -d "$root" ]]; then
      rg --files -g '*.rs' -g '*.js' -g '*.ts' \
        -g '!crates/sema/src/web/assets/**' -g '!playground/src/examples.js' \
        "$root"
    fi
  done | LC_ALL=C sort -u
}

repo_relative_path() {
  local file="$1"
  case "$file" in
    "$repo_root"/*) printf '%s' "${file#"$repo_root"/}" ;;
    *) printf '%s' "$file" ;;
  esac
}

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
scan_purged_paths() {
  cd "$repo_root"
  local files
  files=$(source_files "$@")
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

scan_purged() {
  scan_purged_paths crates/*/src playground/src
}

scan_restricted_paths() {
  cd "$repo_root"
  local files file rel entry token pattern matched line
  files=$(source_files "$@")
  while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    rel=$(repo_relative_path "$file")
    for entry in "${restricted_tokens[@]}"; do
      token="${entry%%|*}"
      pattern="${entry#*|}"
      matched=$(sed -E 's://.*$::; s:;;.*$::' "$file" \
        | rg -n --no-heading --color never "$pattern" || true)
      while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        printf '%s\t%s\t%s\n' "$token" "$rel" "$line"
      done <<< "$matched"
    done
  done <<< "$files"
}

scan_active_runtime_callbacks_paths() {
  cd "$repo_root"
  local files file rel matched token line
  files=$(source_files "$@")
  while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    rel=$(repo_relative_path "$file")
    matched=$(perl -0777 -ne '
      my $source = $_;
      my $code = $source;
      sub blank_non_newlines {
        my $text = shift;
        $text =~ s/[^\n]/ /g;
        return $text;
      }
      $code =~ s{r(#+)".*?"\1}{blank_non_newlines($&)}gse;
      $code =~ s{r".*?"}{blank_non_newlines($&)}gse;
      $code =~ s{"(?:\\.|[^"\\])*"}{blank_non_newlines($&)}gse;
      $code =~ s{`(?:\\.|[^`\\])*`}{blank_non_newlines($&)}gse;
      $code =~ s{\x27(?:\\.|[^\x27\\])\x27}{blank_non_newlines($&)}gse;
      $code =~ s{//[^\n]*}{blank_non_newlines($&)}ge;
      $code =~ s{/\*.*?\*/}{blank_non_newlines($&)}gse;
      $code =~ s{;;[^\n]*}{blank_non_newlines($&)}ge;

      my $guard = qr/(?:(?:[A-Za-z_][A-Za-z0-9_]*)(?:::|\.)\s*)*(?:in_runtime_quantum|runtime_quantum_active)\s*\(\s*\)/;
      while ($code =~ /\bif\b/g) {
        my $start = $-[0];
        my $condition_start = pos($code);
        my ($cursor, $paren_depth, $bracket_depth) = ($condition_start, 0, 0);
        my $body_start;
        while ($cursor < length($code)) {
          my $char = substr($code, $cursor, 1);
          if ($char eq "(") { $paren_depth++; }
          elsif ($char eq ")") { $paren_depth-- if $paren_depth > 0; }
          elsif ($char eq "[") { $bracket_depth++; }
          elsif ($char eq "]") { $bracket_depth-- if $bracket_depth > 0; }
          elsif ($char eq "{" && $paren_depth == 0 && $bracket_depth == 0) {
            $body_start = $cursor;
            last;
          }
          elsif ($char eq ";" && $paren_depth == 0 && $bracket_depth == 0) {
            last;
          }
          $cursor++;
        }
        next unless defined $body_start;

        my $condition = substr($code, $condition_start, $body_start - $condition_start);
        next unless $condition =~ /$guard/;
        (my $compact = $condition) =~ s/\s+//g;
        my $negated_host_only = $compact =~ /^\(*!\(*$guard\)*$/;
        next if $negated_host_only;

        my ($depth, $body_end) = (1, $body_start + 1);
        while ($body_end < length($code) && $depth > 0) {
          my $char = substr($code, $body_end, 1);
          $depth++ if $char eq "{";
          $depth-- if $char eq "}";
          $body_end++;
        }
        next unless $depth == 0;
        my $body = substr($code, $body_start + 1, $body_end - $body_start - 2);
        my $line = 1 + (substr($source, 0, $start) =~ tr/\n//);
        print "EVAL_CALLBACK\t$line:$condition\n"
          if $body =~ /\beval_callback\s*\(/;
        print "CALL_CALLBACK_OWNED\t$line:$condition\n"
          if $body =~ /\bcall_callback_owned\s*\(/;
        print "CALL_CALLBACK\t$line:$condition\n"
          if $body =~ /\bcall_callback\s*\(/;
        print "WITH_STDLIB_CTX\t$line:$condition\n"
          if $body =~ /\bwith_stdlib_ctx\s*(?:<|\()/;
        print "WORKFLOW_TLS\t$line:$condition\n"
          if $body =~ /\bWORKFLOW\.with\b/;
        print "IO_BLOCK_ON\t$line:$condition\n"
          if $body =~ /\bio_block_on\s*\(/;
      }
    ' "$file" || true)
    while IFS=$'\t' read -r token line; do
      [[ -z "$token" ]] && continue
      printf '%s\t%s\t%s\n' "$token" "$rel" "$line"
    done <<< "$matched"
  done <<< "$files"
}

check_restricted_paths() {
  local allowlist="$1"
  shift
  if [[ ! -f "$allowlist" ]]; then
    echo "restricted host-adapter allowlist is missing: $allowlist" >&2
    return 2
  fi

  local hits_file
  hits_file=$(mktemp)
  scan_restricted_paths "$@" >"$hits_file"
  if ! awk -F '\t' '
    BEGIN {
      valid["EVAL_CALLBACK"] = 1
      valid["EVAL_CALLBACK_FN"] = 1
      valid["EVAL_FN"] = 1
      valid["SET_EVAL_CALLBACK"] = 1
      valid["CALL_CALLBACK"] = 1
      valid["CALL_CALLBACK_OWNED"] = 1
      valid["WITH_STDLIB_CTX"] = 1
      valid["SET_CALL_CALLBACK"] = 1
      valid["SET_CALL_OWNED_CALLBACK"] = 1
      valid["WORKFLOW_TLS"] = 1
    }
    FILENAME == ARGV[1] {
      if ($0 ~ /^[[:space:]]*(#|$)/) next
      if (NF != 4 || !($1 in valid) || $2 !~ /^[^[:space:]]+$/ ||
          $3 !~ /^[0-9]+$/ || $4 ~ /^[[:space:]]*$/) {
        print "malformed restricted host-adapter allowlist row: " $0 > "/dev/stderr"
        invalid = 1
        next
      }
      key = $1 SUBSEP $2
      if (key in expected) {
        print "duplicate restricted host-adapter allowlist row: " $1 " " $2 > "/dev/stderr"
        invalid = 1
        next
      }
      expected[key] = $3 + 0
      reason[key] = $4
      next
    }
    {
      key = $1 SUBSEP $2
      actual[key]++
      sample[key] = $2 ":" $3
    }
    END {
      for (key in actual) {
        split(key, part, SUBSEP)
        if (!(key in expected)) {
          print "unallowlisted restricted host adapter " part[1] " at " sample[key] > "/dev/stderr"
          invalid = 1
        } else if (actual[key] != expected[key]) {
          print "restricted host-adapter count changed for " part[1] " in " part[2] \
            ": expected " expected[key] ", found " actual[key] " (" reason[key] ")" > "/dev/stderr"
          invalid = 1
        }
      }
      for (key in expected) {
        if (!(key in actual) && expected[key] != 0) {
          split(key, part, SUBSEP)
          print "restricted host-adapter allowlist entry is stale for " part[1] " in " part[2] \
            ": expected " expected[key] ", found 0 (" reason[key] ")" > "/dev/stderr"
          invalid = 1
        }
      }
      exit invalid
    }
  ' "$allowlist" "$hits_file"; then
    rm -f "$hits_file"
    return 1
  fi
  rm -f "$hits_file"
}

check_source_policy_paths() {
  local allowlist="$1"
  shift
  local purged_hits active_hits failed=0
  purged_hits=$(scan_purged_paths "$@")
  if [[ -n "${purged_hits//[$'\n']/}" ]]; then
    echo "PURGED unified-runtime bridge symbols present in shipped code:" >&2
    echo "$purged_hits" >&2
    failed=1
  fi
  if ! check_restricted_paths "$allowlist" "$@"; then
    failed=1
  fi
  active_hits=$(scan_active_runtime_callbacks_paths "$@")
  if [[ -n "${active_hits//[$'\n']/}" ]]; then
    echo "active-runtime synchronous callback re-entry is prohibited:" >&2
    echo "$active_hits" >&2
    failed=1
  fi
  [[ "$failed" -eq 0 ]]
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
  --scan-restricted)
    scan_restricted_paths crates/*/src playground/src
    ;;
  --check-policy-path)
    if [[ $# -ne 3 ]]; then
      echo "usage: $0 --check-policy-path PATH ALLOWLIST" >&2
      exit 2
    fi
    check_source_policy_paths "$3" "$2"
    ;;
  ""|--check)
    if ! check_source_policy_paths "$host_adapter_allowlist" crates/*/src playground/src; then
      echo "unified-runtime source policy failed" >&2
      exit 1
    fi
    bash "$repo_root/scripts/test-unified-runtime-source-policy.sh"
    echo "ok: unified-runtime deleted bridges are absent and host adapters match their exact allowlist"
    ;;
  *)
    echo "usage: $0 [--check|--scan-purged|--scan-restricted|--scan-path PATH|--check-policy-path PATH ALLOWLIST]" >&2
    exit 2
    ;;
esac
