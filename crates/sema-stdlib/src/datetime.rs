use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use sema_core::{check_arity, SemaError, Value};

use crate::register_fn;

pub fn register(env: &sema_core::Env) {
    register_fn(env, "time/now", |args| {
        check_arity!(args, "time/now", 0);
        let now = Utc::now();
        let secs = now.timestamp() as f64 + now.timestamp_subsec_millis() as f64 / 1000.0;
        Ok(Value::float(secs))
    });

    register_fn(env, "time/format", |args| {
        check_arity!(args, "time/format", 2);
        let ts = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let fmt = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let dt = timestamp_to_datetime(ts)?;
        // chrono's DelayedFormat panics inside Display::to_string on invalid
        // format specifiers; writing through fmt::Write surfaces the error.
        use std::fmt::Write;
        let mut out = String::new();
        write!(out, "{}", dt.format(fmt)).map_err(|_| {
            SemaError::eval(format!("time/format: invalid format string: {fmt:?}")).with_hint(
                "time/format uses chrono format specifiers like %Y-%m-%d %H:%M:%S (see https://docs.rs/chrono/latest/chrono/format/strftime/index.html)",
            )
        })?;
        Ok(Value::string(&out))
    });

    register_fn(env, "time/parse", |args| {
        check_arity!(args, "time/parse", 2);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let fmt = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let naive = NaiveDateTime::parse_from_str(s, fmt).map_err(|e| {
            SemaError::eval(format!("time/parse: parse error: {e}")).with_hint(
                "time/parse uses chrono format specifiers like %Y-%m-%d %H:%M:%S (see https://docs.rs/chrono/latest/chrono/format/strftime/index.html)",
            )
        })?;
        // Intentional: the parsed wall-clock time is interpreted as UTC, not
        // local time, regardless of any offset in the string. Parsing via
        // NaiveDateTime ignores timezone offsets, so callers needing another
        // zone must convert to UTC themselves before parsing. Anchoring to UTC
        // keeps time/parse deterministic and machine-independent.
        let dt: DateTime<Utc> = Utc.from_utc_datetime(&naive);
        Ok(Value::float(dt.timestamp() as f64))
    });

    register_fn(env, "time/date-parts", |args| {
        check_arity!(args, "time/date-parts", 1);
        let ts = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let dt = timestamp_to_datetime(ts)?;
        use chrono::Datelike;
        use chrono::Timelike;
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("year"), Value::int(dt.year() as i64));
        map.insert(Value::keyword("month"), Value::int(dt.month() as i64));
        map.insert(Value::keyword("day"), Value::int(dt.day() as i64));
        map.insert(Value::keyword("hour"), Value::int(dt.hour() as i64));
        map.insert(Value::keyword("minute"), Value::int(dt.minute() as i64));
        map.insert(Value::keyword("second"), Value::int(dt.second() as i64));
        map.insert(
            Value::keyword("weekday"),
            Value::string(&dt.format("%A").to_string()),
        );
        Ok(Value::map(map))
    });

    register_fn(env, "time/add", |args| {
        check_arity!(args, "time/add", 2);
        let ts = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let secs = args[1]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        Ok(Value::float(ts + secs))
    });

    register_fn(env, "time/diff", |args| {
        check_arity!(args, "time/diff", 2);
        let t1 = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let t2 = args[1]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        Ok(Value::float(t1 - t2))
    });
}

fn timestamp_to_datetime(ts: f64) -> Result<DateTime<Utc>, SemaError> {
    if !ts.is_finite() {
        return Err(SemaError::eval("time: invalid timestamp"));
    }
    // Floor toward negative infinity so the nanosecond remainder stays
    // non-negative. A plain `as i64` truncates toward zero, which for negative
    // (pre-1970) timestamps lands in the wrong second — e.g. -0.5 would become
    // 1970-01-01 instead of 1969-12-31 23:59:59.5.
    let secs = ts.floor() as i64;
    let nanos = ((ts - ts.floor()) * 1_000_000_000.0) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .ok_or_else(|| SemaError::eval("time: invalid timestamp"))
}
