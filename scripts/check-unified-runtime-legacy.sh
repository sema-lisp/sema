#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
baseline="$repo_root/docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline"
current="$(mktemp)"
trap 'rm -f "$current"' EXIT

scan_legacy_symbols() {
  cd "$repo_root"
  rg -n --no-heading --color never \
    -g '*.rs' \
    -g '*.js' \
    -g '*.ts' \
    -g '!crates/sema/src/web/assets/**' \
    -g '!playground/src/examples.js' \
    'IoHandle|IoPoll|YieldReason|SchedulerTarget|SchedulerRunResult|set_yield_signal|take_yield_signal|set_resume_value|take_resume_value|set_(eval|call|call_owned|spawn|cancel|run_scheduler)_callback|eval_callback|call_callback(_owned)?|call_(spawn|cancel|run_scheduler)|run_until_reentrant|tasks\.remove\(|in_async_context|io_block_on|\bblock_on\b|thread::sleep|std::thread::sleep|blocking_recv|recv_timeout|HTTP_AWAIT_MARKER|MAX_REPLAYS|XmlHttpRequest|XMLHttpRequest|Atomics::wait|Atomics\.wait|installAtomicsSleep' \
    crates/*/src playground/src \
    | LC_ALL=C sort -u
}

scan_legacy_symbols >"$current"

if [[ ! -s "$current" ]]; then
  echo "legacy scan returned no matches; scanner coverage is broken" >&2
  exit 2
fi

case "${1:-}" in
  --write-baseline)
    mkdir -p "$(dirname "$baseline")"
    cp "$current" "$baseline"
    ;;
  ""|--check)
    if [[ ! -f "$baseline" ]]; then
      echo "legacy baseline is missing; run $0 --write-baseline" >&2
      exit 2
    fi
    cat "$current"
    if ! diff -u "$baseline" "$current" >&2; then
      echo "legacy runtime symbols differ from the committed baseline" >&2
      exit 1
    fi
    ;;
  *)
    echo "usage: $0 [--check|--write-baseline]" >&2
    exit 2
    ;;
esac
