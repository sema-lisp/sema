# Formatter JSON output

`sema fmt --json` is a read-only formatting API for editor and tool integrations. It emits only newline-delimited JSON (NDJSON) on standard output and never rewrites source files.

## Output contract

Each resolved input file produces one JSON object on one line. A successful result keeps the existing fields and adds `changed`, which reports whether the formatted source differs from the input:

```json
{"file":"src/main.sema","formatted":true,"changed":true,"source":"(define x 1)\n"}
```

`formatted` continues to mean that formatting succeeded. Keeping this field preserves compatibility with existing consumers. A failed read or format operation emits the existing error shape:

```json
{"file":"src/main.sema","formatted":false,"error":"..."}
```

Multiple files produce multiple JSON lines. JSON mode emits no human-readable file or aggregate summaries on standard output. If discovery finds no files, standard output is empty. Errors still cause a non-zero exit status after all resolved files have been processed.

The stdin form, `sema fmt --json -`, remains a single-result read-only transform. It has no file path and therefore keeps its existing success and error shapes.

## Option interactions

`--check --json` emits the same per-file JSON results and exits with status 1 when any source would change. It does not rewrite files.

`--diff --json` is rejected by argument parsing. A textual diff and an NDJSON stream cannot share standard output without breaking the machine-readable contract.

## Implementation

File-mode formatting computes `changed` before branching on output mode. JSON mode emits the result and skips file writes; non-JSON mode retains the existing check, diff, write, and summary behavior. Exit-status decisions for formatting errors and `--check` apply in both modes, while human summaries remain exclusive to non-JSON mode.

## Regression coverage

CLI integration tests exercise the real `sema` binary and verify:

- changed and unchanged inputs produce parseable JSON with the correct `changed` value;
- JSON mode leaves source files untouched;
- multi-file output contains one JSON object per line and no summary text;
- `--check --json` exits 1 for unformatted input;
- `--diff --json` is rejected by Clap.
