#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
inventory="$repo_root/docs/internals/async-runtime-inventory.md"
mapping="$repo_root/docs/plans/evidence/unified-cooperative-runtime/runtime-match-map.tsv"
legacy_scanner="$repo_root/scripts/check-unified-runtime-legacy.sh"

discovery_one='IoHandle|IoPoll|YieldReason|Scheduler(Target|RunResult)|run_until_reentrant|call_run_scheduler|set_yield_signal|take_resume_value|in_async_context|io_block_on|block_on|thread_local!'
discovery_two='async/(spawn|await|run|all|race|timeout|sleep|cancel)|channel/(send|recv)|call_callback|eval_callback'

scan_discovery() {
  cd "$repo_root"
  rg -n --with-filename --no-heading --color never \
    -g '*.rs' -g '*.js' -g '*.ts' \
    -g '!crates/sema/src/web/assets/**' \
    -g '!playground/src/examples.js' \
    "$discovery_one" crates/*/src playground/src
  rg -n --with-filename --no-heading --color never \
    -g '*.rs' -g '*.sema' -g '*.js' -g '*.ts' \
    -g '!crates/sema/src/web/assets/**' \
    -g '!playground/src/examples.js' \
    "$discovery_two" crates/*/src playground/src
  "$legacy_scanner" --scan-production
}

current_matches() {
  scan_discovery | LC_ALL=C sort -u
}

row_for_match() {
  local match="$1"
  local path="${match%%:*}"
  local remainder="${match#*:}"
  local line="${remainder%%:*}"
  local row

  case "$path" in
    crates/sema-core/src/async_signal.rs)
      case "$match" in
        *IoPoll*|*"IoHandle::new"*) row=F01B ;;
        *IoHandle*|*io_handle*) row=F01A ;;
        *YieldReason*|*YIELD_SIGNAL*|*yield_signal*) row=F02 ;;
        *SchedulerTarget*|*SchedulerRunResult*|*run_scheduler*) row=F03 ;;
        *SPAWN_CALLBACK*|*CANCEL_CALLBACK*|*spawn_callback*|*cancel_callback*) row=F04 ;;
        *RESUME_VALUE*|*DEBUG_COOP_RESUME*|*resume_value*|*debug_coop_resume*) row=F05 ;;
        *TASK_ID*|*TASK_REAPED*|*task_id*|*task_reaped*) row=F06 ;;
        *blocking_sleep*|*"thread::sleep"*) row=F08B ;;
        *IO_SIGNAL*|*io_park*|*notify_io_complete*|*interrupt*) row=F08A ;;
        *) row=F07 ;;
      esac
      ;;
    crates/sema-core/src/context.rs)
      case "$match" in
        *EvalCallback*|*CallCallback*|*eval_callback*|*call_callback*) row=F10 ;;
        *current_file*|*module_stack*|*loading*) row=F11A ;;
        *module_cache*|*embedded*|*globals*) row=F11B ;;
        *deadline*|*step_limit*) row=F13B ;;
        *sandbox*) row=F13C ;;
        *call_stack*|*span*) row=F13A ;;
        *STDLIB_CTX*|*with_stdlib_ctx*) row=F14 ;;
        *) row=F12 ;;
      esac
      ;;
    crates/sema-core/src/cycle.rs) row=F20 ;;
    crates/sema-core/src/io_backend.rs)
      case "$match" in *io_block_on*|*block_on*) row=F09B ;; *) row=F09A ;; esac
      ;;
    crates/sema-core/src/lib.rs) row=F36 ;;
    crates/sema-core/src/mcp_cassette.rs)
      case "$match" in *decide*|*record*) row=F21B ;; *) row=F21A ;; esac
      ;;
    crates/sema-core/src/output_hook.rs) row=F22 ;;
    crates/sema-core/src/value.rs)
      case "$match" in
        *Channel*|*channel*) row=F16 ;;
        *MutableCell*|*upvalue*) row=F17 ;;
        *NativeFn*|*native*) row=F18 ;;
        *SemaStream*|*StreamBox*) row=F19 ;;
        *) row=F15 ;;
      esac
      ;;
    crates/sema-vm/src/scheduler.rs)
      case "$match" in
        *run_until_reentrant*|*RunGoal*|*run_scheduler*) row=F25 ;;
        *ReinstallGuard*|*tasks.remove*|*capture*|*install*) row=F26 ;;
        *cancel*|*reap*) row=F27 ;;
        *spawn_callback*|*run_closure_as_inline_task*) row=F24 ;;
        *) row=F23 ;;
      esac
      ;;
    crates/sema-vm/src/vm.rs)
      case "$match" in
        *CURRENT_VM*|*call_callback*|*eval_callback*) row=F29 ;;
        *debug*|*DEBUG*) row=F30 ;;
        *) row=F28 ;;
      esac
      ;;
    crates/sema-vm/src/debug.rs) row=F30 ;;
    crates/sema-vm/src/lower.rs) row=C16 ;;
    crates/sema-eval/src/eval.rs)
      case "$match" in *callback*|*call_value*|*eval_value*) row=F32 ;; *) row=F31 ;; esac
      ;;
    crates/sema-eval/src/debug_session.rs) row=F33 ;;
    crates/sema-eval/src/prelude.rs)
      case "$match" in
        *retry*|*timeout*|*sleep*) row=F34B ;;
        *agent*|*stream*) row=F34C ;;
        *async/await*|*async/all*|*async/race*) row=F34D ;;
        *) row=F34A ;;
      esac
      ;;
    crates/sema-io/src/lib.rs)
      case "$match" in *io_block_on*|*block_on*) row=F35B ;; *) row=F35A ;; esac
      ;;

    crates/sema-stdlib/src/async_ops.rs)
      case "$match" in
        *channel*|*Channel*) row=R01C ;;
        *timeout*|*sleep*|*Timer*) row=R01B ;;
        *cancel*|*spawn*|*async_run*) row=R01D ;;
        *) row=R01A ;;
      esac
      ;;
    crates/sema-stdlib/src/archive.rs) row=R02 ;;
    crates/sema-stdlib/src/diff.rs) row=R03 ;;
    crates/sema-stdlib/src/event.rs) row=R04 ;;
    crates/sema-stdlib/src/fs_watch.rs) row=R05 ;;
    crates/sema-stdlib/src/git.rs) row=R06 ;;
    crates/sema-stdlib/src/http.rs)
      case "$match" in *io_block_on*|*block_on*) row=R07B ;; *) row=R07A ;; esac
      ;;
    crates/sema-stdlib/src/io.rs)
      if (( line >= 2680 )); then
        row=V02
      elif [[ "$match" == *call_callback* || "$match" == *eval_callback* ]]; then
        row=R22A
      elif (( line < 1030 )); then
        row=R08C
      elif (( line < 1150 )); then
        row=R08A
      elif (( line < 1327 )); then
        row=R08B
      elif (( line < 1383 )); then
        row=R08C
      elif (( line < 1570 )); then
        row=R08A
      elif (( line < 1633 )); then
        row=R08C
      elif (( line < 2044 )); then
        row=R08A
      elif (( line < 2307 )); then
        row=R08B
      elif (( line < 2372 )); then
        row=R08A
      elif (( line < 2622 )); then
        row=R08C
      else
        row=R08A
      fi
      ;;
    crates/sema-stdlib/src/kv.rs)
      case "$match" in *abort*|*close*|*KV_STORES*|*thread_local*) row=R09A ;; *) row=R09B ;; esac
      ;;
    crates/sema-stdlib/src/pdf.rs) row=R10 ;;
    crates/sema-stdlib/src/proc.rs) row=R11 ;;
    crates/sema-stdlib/src/pty.rs) row=R12 ;;
    crates/sema-stdlib/src/secret.rs) row=R13 ;;
    crates/sema-stdlib/src/serial.rs) row=R14 ;;
    crates/sema-stdlib/src/server.rs)
      case "$match" in *call_callback*|*eval_callback*|*blocking_recv*|*".recv()"*) row=R15B ;; *) row=R15A ;; esac
      ;;
    crates/sema-stdlib/src/sqlite.rs)
      if (( line >= 417 && line < 521 )); then row=R16B; else row=R16A; fi
      ;;
    crates/sema-stdlib/src/stream.rs)
      case "$match" in
        *read_to_end*|*read_all*|*collect*) row=R17C ;;
        *memory*|*string*|*bytes*|*copy*) row=R17B ;;
        *) row=R17A ;;
      esac
      ;;
    crates/sema-stdlib/src/system.rs)
      case "$match" in *signal*|*SIGNAL*) row=R18B ;; *sleep*) row=R18C ;; *) row=R18A ;; esac
      ;;
    crates/sema-stdlib/src/terminal.rs) row=R19 ;;
    crates/sema-stdlib/src/ws.rs)
      case "$match" in *io_block_on*|*block_on*|*blocking_recv*|*".recv()"*) row=R20B ;; *) row=R20A ;; esac
      ;;
    crates/sema-stdlib/src/crypto.rs|crates/sema-stdlib/src/csv_ops.rs|crates/sema-stdlib/src/markup.rs) row=R21 ;;
    crates/sema-stdlib/src/list.rs|crates/sema-stdlib/src/map.rs|crates/sema-stdlib/src/meta.rs)
      row=R22A
      ;;
    crates/sema-stdlib/src/string.rs)
      case "$match" in *thread_local*) row=R22B ;; *) row=R22A ;; esac
      ;;
    crates/sema-stdlib/src/context.rs|crates/sema-stdlib/src/otel.rs|crates/sema-stdlib/src/workflow.rs|crates/sema-stdlib/src/workflow_mcp.rs)
      case "$match" in *call_callback*|*eval_callback*) row=R23A ;; *) row=R23B ;; esac
      ;;

    crates/sema-llm/src/fake.rs) row=C18 ;;
    crates/sema-llm/src/anthropic.rs|crates/sema-llm/src/embeddings.rs|crates/sema-llm/src/gemini.rs|crates/sema-llm/src/ollama.rs|crates/sema-llm/src/openai.rs)
      row=C07D
      ;;
    crates/sema-llm/src/pricing.rs) row=C09 ;;
    crates/sema-llm/src/builtins.rs)
      if (( line >= 8071 )); then
        row=C11
      elif [[ "$match" == *call_callback* || "$match" == *eval_callback* ]]; then
        row=C07C
      elif [[ "$match" == *io_spawn_blocking* || "$match" == *"IoHandle::new"* || "$match" == *io_block_on* || "$match" == *block_on* ]]; then
        row=C07D
      elif (( line < 240 )) && [[ "$match" == *thread_local* || "$match" == *USAGE* || "$match" == *usage* || "$match" == *BUDGET* || "$match" == *budget* ]]; then
        row=C08
      elif (( line >= 6299 && line < 6371 )); then
        row=C06
      elif (( line >= 7190 && line < 7458 )); then
        row=C10
      else
        case "$match" in
          *USAGE*|*usage*|*BUDGET*|*budget*) row=C08 ;;
          *CACHE*|*cache*|*CASSETTE*|*cassette*|*COMPAT*|*pricing*) row=C09 ;;
          *RETRY*|*retry*|*cursor*|*CALL_TAG*|*call_tag*) row=C10 ;;
          *IoHandle*|*io_spawn*|*io_block_on*|*block_on*|*async/*) row=C07B ;;
          *) row=C07A ;;
        esac
      fi
      ;;
    crates/sema-workflow/src/context.rs) row=C12 ;;
    crates/sema-mcp/src/builtins.rs|crates/sema-mcp/src/client_auth.rs|crates/sema-mcp/src/oauth/flow.rs)
      row=C13
      ;;
    crates/sema-mcp/src/oauth/loopback.rs|crates/sema-mcp/src/tools.rs) row=H07 ;;
    crates/sema-otel/src/imp.rs) row=C06 ;;

    crates/sema-dap/src/server.rs) row=H04 ;;
    crates/sema-lsp/src/server.rs|crates/sema-lsp/src/handlers/command.rs) row=H05 ;;
    crates/sema-notebook/src/bridge.rs|crates/sema-notebook/src/engine.rs) row=H06 ;;
    crates/sema/src/lib.rs) row=H01 ;;
    crates/sema/src/main.rs|crates/sema/src/pkg.rs) row=H02 ;;
    crates/sema/src/repl/completer.rs) row=H03 ;;
    crates/sema/src/workflow_mcp.rs|crates/sema/src/web/mod.rs) row=H08 ;;
    crates/sema-wasm/src/lib.rs)
      case "$match" in
        *HTTP_AWAIT_MARKER*|*MAX_REPLAYS*|*replay*) row=H10B ;;
        *XmlHttpRequest*|*XMLHttpRequest*|*Atomics*) row=H10C ;;
        *fetch*|*sleep*|*timer*) row=H10A ;;
        *) row=H09 ;;
      esac
      ;;
    playground/src/sema-worker.js|playground/src/worker-client.js) row=H11 ;;
    playground/src/app.js) row=H12 ;;
    *)
      echo "unmapped production path in runtime inventory: $match" >&2
      return 1
      ;;
  esac

  printf '%s\n' "$row"
}

generate_mapping() {
  local match matches_file row
  matches_file="$(mktemp)"
  if ! current_matches >"$matches_file"; then
    rm -f "$matches_file"
    return 1
  fi
  if [[ ! -s "$matches_file" ]]; then
    rm -f "$matches_file"
    echo "runtime inventory scan returned no production matches" >&2
    return 2
  fi
  while IFS= read -r match; do
    [[ -n "$match" ]] || continue
    row="$(row_for_match "$match")"
    printf '%s\t%s\n' "$row" "$match"
  done <"$matches_file"
  rm -f "$matches_file"
}

check_mapping() {
  local current payload row
  current="$(mktemp)"
  payload="$(mktemp)"
  trap "rm -f '$current' '$payload'" EXIT

  current_matches >"$current"
  if [[ ! -s "$current" ]]; then
    echo "runtime inventory scan returned no production matches" >&2
    exit 2
  fi
  if [[ ! -f "$mapping" ]]; then
    echo "runtime inventory mapping is missing; run $0 --write-mapping" >&2
    exit 2
  fi
  if ! awk '
    BEGIN { FS = "\t" }
    NF < 2 || $1 !~ /^[A-Z][0-9][0-9][A-Z]?$/ || $2 !~ /^[^:]+:[0-9]+:/ { exit 1 }
  ' "$mapping"; then
    echo "runtime inventory mapping contains a malformed row" >&2
    exit 1
  fi

  cut -f2- "$mapping" >"$payload"
  if ! LC_ALL=C sort -c -u "$payload"; then
    echo "runtime inventory mapping payload is not sorted and unique" >&2
    exit 1
  fi
  if ! diff -u "$payload" "$current"; then
    echo "runtime inventory mapping has missing or stale exact matches" >&2
    exit 1
  fi

  while IFS= read -r row; do
    if ! grep -Fq "| $row " "$inventory"; then
      echo "runtime inventory mapping references missing ledger row $row" >&2
      exit 1
    fi
  done < <(cut -f1 "$mapping" | LC_ALL=C sort -u)

  printf 'runtime inventory mapping covers %s exact production matches\n' "$(wc -l <"$mapping" | tr -d ' ')"
}

case "${1:---check}" in
  --check)
    check_mapping
    ;;
  --write-mapping)
    next_mapping="$(mktemp)"
    trap 'rm -f "$next_mapping"' EXIT
    mkdir -p "$(dirname "$mapping")"
    generate_mapping >"$next_mapping"
    mv "$next_mapping" "$mapping"
    ;;
  *)
    echo "usage: $0 [--check|--write-mapping]" >&2
    exit 2
    ;;
esac
