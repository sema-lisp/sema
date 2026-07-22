//! CSV parse/encode builtins.
//!
//! **Bounded / offloaded CPU (B9 R21 split).** `csv/parse` and `csv/parse-maps`
//! materialize a row-per-record table, so during a runtime quantum
//! (`in_runtime_quantum()`) they capture a per-input byte cap BEFORE dispatch and
//! offload the parse onto the I/O pool through `quarantined_compute` (`io.rs`, the
//! same mechanism `archive.rs`/`diff.rs`/`secret.rs` use). The parse runs over an
//! owned `String` snapshot (`Send`) on a worker and returns `Send` cell strings
//! (`Vec<Vec<String>>`, or `(headers, rows)` for the map form); the `Value` table
//! is built back on the VM thread. The worker also enforces incremental row/cell
//! caps so a hostile record count is rejected at boundary+1. No `Value`/`Env`
//! crosses the thread boundary. `csv/encode` reads `Value` rows already in memory,
//! so it stays SYNCHRONOUS with a pre-dispatch row-count cap inside a quantum
//! (bounded rows ⇒ bounded VM-thread CPU) — an explicit synchronous split, not a
//! fake async wrap. A direct native call outside the cooperative runtime keeps the
//! uncapped synchronous shape.

use std::collections::BTreeMap;

use sema_core::{check_arity, SemaError, Value, ValueView};

use crate::register_fn;
#[cfg(not(target_arch = "wasm32"))]
use std::cell::Cell;
#[cfg(not(target_arch = "wasm32"))]
use {crate::register_runtime_fn, sema_core::runtime::NativeOutcome};

/// Per-input byte cap for `csv/parse`(`-maps`) under a runtime quantum. CSV
/// parsing is O(input); 64 MiB is far above any realistic table.
#[cfg(not(target_arch = "wasm32"))]
const CSV_INPUT_BYTE_CAP: u64 = 64 * 1024 * 1024;

/// Row-count cap enforced incrementally on the worker (and the `csv/encode`
/// pre-dispatch cap). Generous relative to the byte cap; a guardrail against a
/// pathological record count, not the terminal bound.
const CSV_MAX_ROWS: usize = 5_000_000;
/// Cell-per-row cap enforced incrementally on the worker.
const CSV_MAX_CELLS_PER_ROW: usize = 1_000_000;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// Optional per-call input-byte cap override (lowered, never raised above the
    /// hard ceiling). Read on the VM thread pre-dispatch; mirrors
    /// `diff::DIFF_INPUT_BYTE_CAP_OVERRIDE`. `None` uses the module ceiling. The
    /// seam the regression suite drives to exercise the cap boundary without a
    /// multi-megabyte input string.
    static CSV_INPUT_BYTE_CAP_OVERRIDE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The effective per-input byte cap for the current call: the module ceiling,
/// lowered by any per-call override (never raised above it).
#[cfg(not(target_arch = "wasm32"))]
fn effective_csv_input_byte_cap() -> u64 {
    CSV_INPUT_BYTE_CAP_OVERRIDE
        .with(Cell::get)
        .map_or(CSV_INPUT_BYTE_CAP, |over| over.min(CSV_INPUT_BYTE_CAP))
}

/// Lower the per-input byte cap (clamped to the hard ceiling) for subsequent
/// `csv/parse` calls on this thread, or clear the override with `None`. Test
/// seam, mirroring `set_diff_input_byte_cap_override`.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_csv_input_byte_cap_override(bytes: Option<u64>) {
    CSV_INPUT_BYTE_CAP_OVERRIDE.with(|cell| cell.set(bytes));
}

/// Reject `actual` over `limit`. Reads an existing `len()`/count — no snapshot —
/// so an over-cap input is rejected without any excess allocation.
#[cfg(not(target_arch = "wasm32"))]
fn check_csv_limit(op: &str, dimension: &str, actual: u64, limit: u64) -> Result<(), SemaError> {
    if actual > limit {
        return Err(SemaError::eval(format!(
            "{op}: {dimension} {actual} exceeds the quarantined limit {limit}"
        ))
        .with_hint("reduce or split the CSV input"));
    }
    Ok(())
}

/// Parse `s` into a table of owned cell strings, enforcing the row/cell caps
/// incrementally (rejecting at boundary+1). Shared by the synchronous and
/// offloaded `csv/parse` paths; the result is `Send`, so it can cross the offload
/// thread boundary (the `Value` table is built by [`csv_rows_to_value`]).
fn csv_parse_work(
    s: &str,
    row_cap: usize,
    cell_cap: usize,
) -> Result<Vec<Vec<String>>, SemaError> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(s.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    for result in rdr.records() {
        let record = result.map_err(|e| {
            SemaError::eval(format!("csv/parse: parse error: {e}"))
                .with_hint("csv/parse expects comma-separated values; check delimiters and quoting")
        })?;
        if rows.len() >= row_cap {
            return Err(SemaError::eval(format!(
                "csv/parse: rows {} exceeds the quarantined limit {row_cap}",
                row_cap + 1
            ))
            .with_hint("reduce or split the CSV input"));
        }
        let mut row: Vec<String> = Vec::with_capacity(record.len());
        for field in record.iter() {
            if row.len() >= cell_cap {
                return Err(SemaError::eval(format!(
                    "csv/parse: cells {} exceeds the quarantined limit {cell_cap}",
                    cell_cap + 1
                ))
                .with_hint("reduce or split the CSV input"));
            }
            row.push(field.to_string());
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Parse `s` with a header row, returning `(headers, rows)` as owned strings. Same
/// caps and offload rationale as [`csv_parse_work`].
fn csv_parse_maps_work(
    s: &str,
    row_cap: usize,
    cell_cap: usize,
) -> Result<(Vec<String>, Vec<Vec<String>>), SemaError> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(s.as_bytes());
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| {
            SemaError::eval(format!("csv/parse-maps: parse error: {e}")).with_hint(
                "csv/parse-maps expects a header row followed by comma-separated values",
            )
        })?
        .iter()
        .map(|h| h.to_string())
        .collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for result in rdr.records() {
        let record = result.map_err(|e| {
            SemaError::eval(format!("csv/parse-maps: parse error: {e}")).with_hint(
                "csv/parse-maps expects comma-separated values; check delimiters and quoting",
            )
        })?;
        if rows.len() >= row_cap {
            return Err(SemaError::eval(format!(
                "csv/parse-maps: rows {} exceeds the quarantined limit {row_cap}",
                row_cap + 1
            ))
            .with_hint("reduce or split the CSV input"));
        }
        let mut row: Vec<String> = Vec::with_capacity(record.len());
        for field in record.iter() {
            if row.len() >= cell_cap {
                return Err(SemaError::eval(format!(
                    "csv/parse-maps: cells {} exceeds the quarantined limit {cell_cap}",
                    cell_cap + 1
                ))
                .with_hint("reduce or split the CSV input"));
            }
            row.push(field.to_string());
        }
        rows.push(row);
    }
    Ok((headers, rows))
}

/// Build `csv/parse`'s list-of-rows `Value` from the `Send` cell table. A plain
/// `fn` (no captures) so it fits `quarantined_compute`'s `fn(T) -> Value` slot.
fn csv_rows_to_value(rows: Vec<Vec<String>>) -> Value {
    let out: Vec<Value> = rows
        .into_iter()
        .map(|row| Value::list(row.iter().map(|c| Value::string(c)).collect()))
        .collect();
    Value::list(out)
}

/// Build `csv/parse-maps`'s list-of-maps `Value` from the `Send` `(headers, rows)`
/// pair. Fields beyond the header count are dropped (keying by header index),
/// matching the original synchronous shape.
fn csv_maps_to_value(pair: (Vec<String>, Vec<Vec<String>>)) -> Value {
    let (headers, rows) = pair;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let mut map = BTreeMap::new();
            for (i, field) in row.iter().enumerate() {
                if let Some(header) = headers.get(i) {
                    map.insert(Value::keyword(header), Value::string(field));
                }
            }
            Value::map(map)
        })
        .collect();
    Value::list(out)
}

pub fn register(env: &sema_core::Env) {
    // (csv/parse text) -> list of rows (each a list of cell strings). CPU-bound
    // table materialization; in a runtime quantum it captures a per-input byte cap
    // BEFORE dispatch and offloads onto the I/O pool via `quarantined_compute`.
    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn(env, "csv/parse", |args| {
        check_arity!(args, "csv/parse", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_csv_limit("csv/parse", "input bytes", s.len() as u64, effective_csv_input_byte_cap())?;
            let snapshot = s.to_string();
            return crate::io::quarantined_compute("csv/parse", csv_rows_to_value, move || {
                csv_parse_work(&snapshot, CSV_MAX_ROWS, CSV_MAX_CELLS_PER_ROW)
                    .map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(csv_rows_to_value(csv_parse_work(
            s,
            CSV_MAX_ROWS,
            CSV_MAX_CELLS_PER_ROW,
        )?)))
    });
    #[cfg(target_arch = "wasm32")]
    register_fn(env, "csv/parse", |args| {
        check_arity!(args, "csv/parse", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(csv_rows_to_value(csv_parse_work(
            s,
            CSV_MAX_ROWS,
            CSV_MAX_CELLS_PER_ROW,
        )?))
    });

    // (csv/parse-maps text) -> list of maps keyed by the header row.
    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn(env, "csv/parse-maps", |args| {
        check_arity!(args, "csv/parse-maps", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        if sema_core::in_runtime_quantum() {
            check_csv_limit(
                "csv/parse-maps",
                "input bytes",
                s.len() as u64,
                effective_csv_input_byte_cap(),
            )?;
            let snapshot = s.to_string();
            return crate::io::quarantined_compute("csv/parse-maps", csv_maps_to_value, move || {
                csv_parse_maps_work(&snapshot, CSV_MAX_ROWS, CSV_MAX_CELLS_PER_ROW)
                    .map_err(|e| e.to_string())
            });
        }
        Ok(NativeOutcome::Return(csv_maps_to_value(csv_parse_maps_work(
            s,
            CSV_MAX_ROWS,
            CSV_MAX_CELLS_PER_ROW,
        )?)))
    });
    #[cfg(target_arch = "wasm32")]
    register_fn(env, "csv/parse-maps", |args| {
        check_arity!(args, "csv/parse-maps", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(csv_maps_to_value(csv_parse_maps_work(
            s,
            CSV_MAX_ROWS,
            CSV_MAX_CELLS_PER_ROW,
        )?))
    });

    // (csv/encode rows) -> CSV string. Reads `Value` rows already in memory, so it
    // stays synchronous; inside a runtime quantum a pre-dispatch row-count cap
    // keeps its VM-thread CPU bounded (a synchronous split, not a fake async wrap).
    register_fn(env, "csv/encode", |args| {
        check_arity!(args, "csv/encode", 1);
        let rows = match args[0].view() {
            ValueView::List(l) => l.as_ref().clone(),
            _ => return Err(SemaError::type_error("list", args[0].type_name())),
        };
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_runtime_quantum() {
            check_csv_limit("csv/encode", "rows", rows.len() as u64, CSV_MAX_ROWS as u64)?;
        }
        let mut wtr = csv::WriterBuilder::new().from_writer(Vec::new());
        for row in &rows {
            let fields: Vec<String> = match row.view() {
                ValueView::List(l) => l
                    .iter()
                    .map(|v| match v.as_str() {
                        Some(s) => s.to_string(),
                        None => v.to_string(),
                    })
                    .collect(),
                ValueView::Vector(v) => v
                    .iter()
                    .map(|val| match val.as_str() {
                        Some(s) => s.to_string(),
                        None => val.to_string(),
                    })
                    .collect(),
                _ => return Err(SemaError::type_error("list", row.type_name())),
            };
            wtr.write_record(&fields)
                .map_err(|e| SemaError::eval(format!("csv/encode: {e}")))?;
        }
        let bytes = wtr
            .into_inner()
            .map_err(|e| SemaError::eval(format!("csv/encode: {e}")))?;
        let s =
            String::from_utf8(bytes).map_err(|e| SemaError::eval(format!("csv/encode: {e}")))?;
        Ok(Value::string(&s))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_work_round_trips_cells() {
        let rows = csv_parse_work("a,b\nc,d\n", CSV_MAX_ROWS, CSV_MAX_CELLS_PER_ROW).unwrap();
        assert_eq!(
            rows,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string(), "d".to_string()],
            ]
        );
    }

    #[test]
    fn parse_work_rejects_row_and_cell_caps_at_boundary_plus_one() {
        // Two rows at the row cap = 2 are accepted; a third is rejected.
        assert!(csv_parse_work("1\n2\n", 2, 8).is_ok());
        let err = csv_parse_work("1\n2\n3\n", 2, 8).expect_err("third row is one over");
        assert!(err.to_string().contains("rows"), "{err}");

        // Two cells at the cell cap = 2 are accepted; a third is rejected.
        assert!(csv_parse_work("a,b\n", 8, 2).is_ok());
        let err = csv_parse_work("a,b,c\n", 8, 2).expect_err("third cell is one over");
        assert!(err.to_string().contains("cells"), "{err}");
    }

    #[test]
    fn maps_work_keys_rows_by_header() {
        let (headers, rows) =
            csv_parse_maps_work("h1,h2\nx,y\n", CSV_MAX_ROWS, CSV_MAX_CELLS_PER_ROW).unwrap();
        assert_eq!(headers, vec!["h1".to_string(), "h2".to_string()]);
        assert_eq!(rows, vec![vec!["x".to_string(), "y".to_string()]]);
        let value = csv_maps_to_value((headers, rows));
        let list = value.as_list().unwrap();
        assert_eq!(list.len(), 1);
        let map = list[0].as_map_ref().unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&Value::keyword("h1")).and_then(|v| v.as_str()),
            Some("x")
        );
    }

    /// The decoder keys strictly by header index, so an overflow cell with no
    /// header is dropped (matching the original synchronous shape).
    #[test]
    fn maps_to_value_drops_overflow_cell() {
        let headers = vec!["h1".to_string(), "h2".to_string()];
        let rows = vec![vec!["x".to_string(), "y".to_string(), "z".to_string()]];
        let value = csv_maps_to_value((headers, rows));
        let map = value.as_list().unwrap()[0].as_map_ref().unwrap();
        assert_eq!(map.len(), 2, "the headerless overflow cell z is dropped");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn csv_limit_accepts_boundary_and_rejects_one_over() {
        assert!(check_csv_limit("csv/parse", "input bytes", 8, 8).is_ok());
        let error = check_csv_limit("csv/parse", "input bytes", 9, 8)
            .expect_err("one byte over the captured limit must fail");
        assert!(error.to_string().contains('9'));
        assert!(error.to_string().contains('8'));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn csv_input_byte_cap_is_finite_and_clamps_overrides() {
        assert_eq!(effective_csv_input_byte_cap(), CSV_INPUT_BYTE_CAP);
        set_csv_input_byte_cap_override(Some(16));
        assert_eq!(effective_csv_input_byte_cap(), 16);
        // An override above the hard ceiling is clamped down, never raised.
        set_csv_input_byte_cap_override(Some(u64::MAX));
        assert_eq!(effective_csv_input_byte_cap(), CSV_INPUT_BYTE_CAP);
        set_csv_input_byte_cap_override(None);
        assert_eq!(effective_csv_input_byte_cap(), CSV_INPUT_BYTE_CAP);
    }
}
