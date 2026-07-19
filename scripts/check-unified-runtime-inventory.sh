#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
inventory="${UNIFIED_RUNTIME_INVENTORY_FILE:-$repo_root/docs/internals/async-runtime-inventory.md}"
mapping="${UNIFIED_RUNTIME_MAPPING_FILE:-$repo_root/docs/plans/evidence/unified-cooperative-runtime/runtime-match-map.tsv}"
legacy_scanner="$repo_root/scripts/check-unified-runtime-legacy.sh"
rg_bin="${UNIFIED_RUNTIME_RG_BIN:-rg}"

discovery_one='IoHandle|IoPoll|YieldReason|Scheduler(Target|RunResult)|run_until_reentrant|call_run_scheduler|set_yield_signal|take_resume_value|in_async_context|io_block_on|block_on|thread_local!'
discovery_two='async/(spawn|await|run|all|race|timeout|sleep|cancel)|channel/(send|recv)|call_callback|eval_callback'

usage() {
  echo "usage: $0 [--check|--write-mapping|--check-files MAPPING CURRENT INVENTORY]" >&2
  exit 2
}

scan_discovery() {
  cd "$repo_root"
  if ! "$rg_bin" -n --with-filename --no-heading --color never \
    -g '*.rs' -g '*.js' -g '*.ts' \
    -g '!crates/sema/src/web/assets/**' \
    -g '!playground/src/examples.js' \
    "$discovery_one" crates/*/src playground/src; then
    return 1
  fi
  if ! "$rg_bin" -n --with-filename --no-heading --color never \
    -g '*.rs' -g '*.sema' -g '*.js' -g '*.ts' \
    -g '!crates/sema/src/web/assets/**' \
    -g '!playground/src/examples.js' \
    "$discovery_two" crates/*/src playground/src; then
    return 1
  fi
  "$legacy_scanner" --scan-production
}

current_matches() {
  scan_discovery | LC_ALL=C sort -u
}

check_mapping_files() {
  local mapping_file="$1"
  local current_file="$2"
  local inventory_file="$3"
  local payload_file ledger_status_file mapped_row_file

  if [[ ! -s "$current_file" ]]; then
    echo "runtime inventory scan returned no production matches" >&2
    return 2
  fi
  if [[ ! -s "$mapping_file" ]]; then
    echo "runtime inventory mapping is missing or empty" >&2
    return 2
  fi
  if [[ ! -f "$inventory_file" ]]; then
    echo "runtime inventory ledger is missing: $inventory_file" >&2
    return 2
  fi
  if ! LC_ALL=C sort -c -u "$current_file"; then
    echo "runtime inventory current-match input is not sorted and unique" >&2
    return 1
  fi
  if ! awk '
    BEGIN { FS = "\t" }
    NF < 2 || ($1 != "UNREVIEWED" && $1 !~ /^[A-Z][0-9][0-9][A-Z]?$/) ||
      substr($0, index($0, "\t") + 1) !~ /^[^:]+:[0-9]+:/ { exit 1 }
  ' "$mapping_file"; then
    echo "runtime inventory mapping contains a malformed row" >&2
    return 1
  fi
  if awk -F '\t' '$1 == "UNREVIEWED" { found = 1 } END { exit !found }' "$mapping_file"; then
    echo "runtime inventory mapping contains UNREVIEWED assignments" >&2
    return 1
  fi

  payload_file="$(mktemp)"
  ledger_status_file="$(mktemp)"
  mapped_row_file="$(mktemp)"
  cut -f2- "$mapping_file" >"$payload_file"
  if ! LC_ALL=C sort -c -u "$payload_file"; then
    rm -f "$payload_file" "$ledger_status_file" "$mapped_row_file"
    echo "runtime inventory mapping payload is not sorted and unique" >&2
    return 1
  fi
  if ! diff -u "$payload_file" "$current_file"; then
    rm -f "$payload_file" "$ledger_status_file" "$mapped_row_file"
    echo "runtime inventory mapping has missing or stale exact matches" >&2
    return 1
  fi

  awk -F '|' '
    /^\|/ {
      cell = $2
      sub(/^[[:space:]]+/, "", cell)
      sub(/[[:space:]]+$/, "", cell)
      split(cell, parts, /[[:space:]]+/)
      if (parts[1] ~ /^[A-Z][0-9][0-9][A-Z]?$/) {
        status = $(NF - 1)
        sub(/^[[:space:]]+/, "", status)
        sub(/[[:space:]]+$/, "", status)
        print parts[1] "\t" status
      }
    }
  ' "$inventory_file" | LC_ALL=C sort -u >"$ledger_status_file"
  cut -f1 "$mapping_file" | LC_ALL=C sort -u >"$mapped_row_file"
  if ! awk -F '\t' '
    NR == FNR { ledger[$1] = $2; next }
    !($1 in ledger) {
      print "runtime inventory mapping references missing ledger row " $1 > "/dev/stderr"
      invalid = 1
      next
    }
    ledger[$1] != "MIGRATED" && ledger[$1] != "REMOVED" &&
      ledger[$1] != "SYNCHRONOUS-PROOF" {
      print "runtime inventory mapping references nonterminal ledger row " $1 \
        " (" ledger[$1] ")" > "/dev/stderr"
      invalid = 1
    }
    END { exit invalid }
  ' "$ledger_status_file" "$mapped_row_file"; then
    rm -f "$payload_file" "$ledger_status_file" "$mapped_row_file"
    return 1
  fi

  rm -f "$payload_file" "$ledger_status_file" "$mapped_row_file"
  printf 'runtime inventory mapping covers %s exact production matches\n' \
    "$(wc -l <"$mapping_file" | tr -d ' ')"
}

write_mapping() {
  local current_file previous_file next_mapping
  current_file="$(mktemp)"
  previous_file="$(mktemp)"
  next_mapping="$(mktemp)"
  trap 'rm -f "$current_file" "$previous_file" "$next_mapping"' RETURN

  if ! current_matches >"$current_file"; then
    echo "runtime inventory discovery scan failed" >&2
    return 1
  fi
  if [[ ! -s "$current_file" ]]; then
    echo "runtime inventory scan returned no production matches" >&2
    return 2
  fi
  if [[ -f "$mapping" ]]; then
    cp "$mapping" "$previous_file"
  fi

  if ! awk -F '\t' '
    BEGIN { OFS = "\t" }
    FILENAME == ARGV[1] {
      row = $1
      payload = substr($0, index($0, "\t") + 1)
      if (NF < 2 || (row != "UNREVIEWED" && row !~ /^[A-Z][0-9][0-9][A-Z]?$/) ||
          payload !~ /^[^:]+:[0-9]+:/ || payload in assigned) {
        invalid = 1
        next
      }
      assigned[payload] = row
      next
    }
    {
      row = ($0 in assigned) ? assigned[$0] : "UNREVIEWED"
      print row, $0
    }
    END { exit invalid }
  ' "$previous_file" "$current_file" >"$next_mapping"; then
    echo "existing runtime inventory mapping is malformed or contains duplicate payloads" >&2
    return 1
  fi

  mkdir -p "$(dirname "$mapping")"
  mv "$next_mapping" "$mapping"
}

case "${1:---check}" in
  --check)
    [[ $# -eq 1 ]] || usage
    current_file="$(mktemp)"
    trap 'rm -f "$current_file"' EXIT
    if ! current_matches >"$current_file"; then
      echo "runtime inventory discovery scan failed" >&2
      exit 1
    fi
    check_mapping_files "$mapping" "$current_file" "$inventory"
    ;;
  --write-mapping)
    [[ $# -eq 1 ]] || usage
    write_mapping
    ;;
  --check-files)
    [[ $# -eq 4 ]] || usage
    check_mapping_files "$2" "$3" "$4"
    ;;
  *)
    usage
    ;;
esac
