use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};

use sema_core::cycle::GcEdge;
use sema_core::number::SemaNumber;
use sema_core::runtime::{
    NativeCall, NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, ResumeInput,
    Trace,
};
use sema_core::{check_arity, intern, Record, SemaError, Value, ValueViewRef};

use crate::register_fn;

/// Continuation state machine that drives a `map` callback COOPERATIVELY under
/// the unified runtime (Task 04). `map`, when it runs inside a runtime quantum,
/// returns `NativeOutcome::Call{ callback, [item0], MapContinuation }`; the
/// runtime runs `callback(item0)` as real Sema work on the active task (so an
/// async op inside it parks and resumes), then resumes this continuation with the
/// result. Each resume either issues the next `Call` or, once every item is
/// mapped, `Return`s the assembled list — one fresh cooperative call per element.
struct MapContinuation {
    callback: Value,
    remaining: VecDeque<Value>,
    results: Vec<Value>,
}

impl Trace for MapContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.callback));
        for item in &self.remaining {
            sink(GcEdge::Value(item));
        }
        for result in &self.results {
            sink(GcEdge::Value(result));
        }
        true
    }
}

impl NativeContinuation for MapContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let value = match input {
            ResumeInput::Returned(value) => value,
            // A callback error / cancellation aborts the whole `map`: propagate it
            // so the runtime resumes the parked parent VM with the raised error
            // (catchable by an enclosing try/catch), matching the legacy path's
            // fail-fast on the first erroring element.
            ResumeInput::Failed(error) => return Err(error),
            ResumeInput::Cancelled(reason) => {
                return Err(SemaError::eval(format!(
                    "map callback was cancelled ({reason:?})"
                )))
            }
            ResumeInput::Runtime(_) => {
                return Err(SemaError::eval(
                    "map continuation received an unexpected runtime response",
                ))
            }
        };
        self.results.push(value);
        match self.remaining.pop_front() {
            Some(next) => {
                let callable = self.callback.clone();
                Ok(NativeOutcome::Call(NativeCall {
                    callable,
                    args: vec![next],
                    continuation: self,
                }))
            }
            None => Ok(NativeOutcome::Return(Value::list(std::mem::take(
                &mut self.results,
            )))),
        }
    }
}

/// Build the initial cooperative `NativeOutcome::Call` for a single-list `map`
/// running inside a runtime quantum: run `callback(item0)` as real Sema work on
/// the active task, with `MapContinuation` driving the rest. Empty input has no
/// callback to run, so it returns the empty list directly.
fn map_call(callback: &Value, items: &[Value]) -> NativeOutcome {
    let Some((first, rest)) = items.split_first() else {
        return NativeOutcome::Return(Value::list(Vec::new()));
    };
    let continuation = Box::new(MapContinuation {
        callback: callback.clone(),
        remaining: rest.iter().cloned().collect(),
        results: Vec::with_capacity(items.len()),
    });
    NativeOutcome::Call(NativeCall {
        callable: callback.clone(),
        args: vec![first.clone()],
        continuation,
    })
}

/// Synchronous value-ABI implementation for multi-list `map`: iterate the lists
/// in lockstep (shortest wins), calling the callback on each column.
fn map_multi(args: &[Value]) -> Result<Value, SemaError> {
    let lists: Vec<Cow<[Value]>> = args[1..]
        .iter()
        .map(|a| get_sequence(a, "map"))
        .collect::<Result<_, _>>()?;
    let min_len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
    let mut result = Vec::with_capacity(min_len);
    for i in 0..min_len {
        let call_args: Vec<Value> = lists.iter().map(|l| l[i].clone()).collect();
        result.push(call_function(&args[0], &call_args)?);
    }
    Ok(Value::list(result))
}

/// Pending inputs for a predicate scan. Immutable lists and vectors are kept as
/// one traced source handle; mutable arrays use the snapshot produced by
/// `get_sequence` so callbacks may mutate the original array safely.
enum PredicateItems {
    Retained { source: Value, next: usize },
    Snapshot { items: Vec<Value>, next: usize },
}

impl PredicateItems {
    fn from_value(source: &Value, hof: &'static str) -> Result<Self, SemaError> {
        match get_sequence(source, hof)? {
            Cow::Borrowed(_) => Ok(Self::Retained {
                source: source.clone(),
                next: 0,
            }),
            Cow::Owned(items) => Ok(Self::Snapshot { items, next: 0 }),
        }
    }

    fn pop_front(&mut self) -> Option<Value> {
        match self {
            Self::Retained { source, next } => {
                let item = source
                    .as_list()
                    .or_else(|| source.as_vector())
                    .and_then(|items| items.get(*next))
                    .cloned();
                *next += usize::from(item.is_some());
                item
            }
            Self::Snapshot { items, next } if *next < items.len() => {
                let item = std::mem::replace(&mut items[*next], Value::nil());
                *next += 1;
                Some(item)
            }
            Self::Snapshot { .. } => None,
        }
    }

    fn remaining_len(&self) -> usize {
        match self {
            Self::Retained { source, next } => source
                .as_list()
                .or_else(|| source.as_vector())
                .map_or(0, |items| items.len().saturating_sub(*next)),
            Self::Snapshot { items, next } => items.len().saturating_sub(*next),
        }
    }

    fn take_remaining(&mut self) -> Vec<Value> {
        match self {
            Self::Retained { source, next } => {
                let remaining = source
                    .as_list()
                    .or_else(|| source.as_vector())
                    .map_or_else(Vec::new, |items| items[*next..].to_vec());
                *next += remaining.len();
                remaining
            }
            Self::Snapshot { items, next } => {
                let remaining = items.drain(*next..).collect();
                *next = items.len();
                remaining
            }
        }
    }

    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        match self {
            Self::Retained { source, .. } => sink(GcEdge::Value(source)),
            Self::Snapshot { items, next } => {
                for item in &items[*next..] {
                    sink(GcEdge::Value(item));
                }
            }
        }
    }
}

/// Result policy and accumulated state for a predicate-driven sequence scan.
enum PredicateMode {
    Select {
        keep_when_truthy: bool,
        results: Vec<Value>,
    },
    Partition {
        matching: Vec<Value>,
        non_matching: Vec<Value>,
    },
    Any,
    Every,
    TakeWhile {
        results: Vec<Value>,
    },
    DropWhile,
    Find,
    Sole {
        found: Option<Value>,
    },
}

impl PredicateMode {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        match self {
            Self::Select { results, .. } | Self::TakeWhile { results } => {
                for result in results {
                    sink(GcEdge::Value(result));
                }
            }
            Self::Partition {
                matching,
                non_matching,
            } => {
                for result in matching {
                    sink(GcEdge::Value(result));
                }
                for result in non_matching {
                    sink(GcEdge::Value(result));
                }
            }
            Self::Sole { found: Some(found) } => sink(GcEdge::Value(found)),
            Self::Any | Self::Every | Self::DropWhile | Self::Find | Self::Sole { found: None } => {
            }
        }
    }

    fn finish(&mut self, hof: &'static str) -> Result<Value, SemaError> {
        match self {
            Self::Select { results, .. } | Self::TakeWhile { results } => {
                Ok(Value::list(std::mem::take(results)))
            }
            Self::Partition {
                matching,
                non_matching,
            } => Ok(Value::list(vec![
                Value::list(std::mem::take(matching)),
                Value::list(std::mem::take(non_matching)),
            ])),
            Self::Any => Ok(Value::bool(false)),
            Self::Every => Ok(Value::bool(true)),
            Self::DropWhile => Ok(Value::list(Vec::new())),
            Self::Find => Ok(Value::nil()),
            Self::Sole { found } => found
                .take()
                .ok_or_else(|| SemaError::eval(format!("{hof}: no matching item"))),
        }
    }
}

/// Cooperative state machine for predicate higher-order functions. The current
/// input is retained separately because `find`, `sole`, and the selecting modes
/// must return the exact `Value` passed to the predicate.
struct PredicateContinuation {
    hof: &'static str,
    predicate: Value,
    current: Value,
    remaining: PredicateItems,
    mode: PredicateMode,
}

impl PredicateContinuation {
    fn continue_or_finish(mut self: Box<Self>) -> NativeResult {
        match self.remaining.pop_front() {
            Some(next) => {
                self.current = next.clone();
                Ok(NativeOutcome::Call(NativeCall {
                    callable: self.predicate.clone(),
                    args: vec![next],
                    continuation: self,
                }))
            }
            None => self.mode.finish(self.hof).map(NativeOutcome::Return),
        }
    }
}

impl Trace for PredicateContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.predicate));
        sink(GcEdge::Value(&self.current));
        self.remaining.trace(sink);
        self.mode.trace(sink);
        true
    }
}

impl NativeContinuation for PredicateContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let matches = resume_value(input, self.hof)?.is_truthy();

        match &mut self.mode {
            PredicateMode::Select {
                keep_when_truthy,
                results,
            } => {
                if matches == *keep_when_truthy {
                    results.push(self.current.clone());
                }
            }
            PredicateMode::Partition {
                matching,
                non_matching,
            } => {
                if matches {
                    matching.push(self.current.clone());
                } else {
                    non_matching.push(self.current.clone());
                }
            }
            PredicateMode::Any if matches => return Ok(NativeOutcome::Return(Value::bool(true))),
            PredicateMode::Every if !matches => {
                return Ok(NativeOutcome::Return(Value::bool(false)))
            }
            PredicateMode::TakeWhile { results } if matches => {
                results.push(self.current.clone());
            }
            PredicateMode::TakeWhile { results } => {
                return Ok(NativeOutcome::Return(Value::list(std::mem::take(results))))
            }
            PredicateMode::DropWhile if !matches => {
                let mut results = Vec::with_capacity(self.remaining.remaining_len() + 1);
                results.push(std::mem::replace(&mut self.current, Value::nil()));
                results.extend(self.remaining.take_remaining());
                return Ok(NativeOutcome::Return(Value::list(results)));
            }
            PredicateMode::Find if matches => {
                return Ok(NativeOutcome::Return(std::mem::replace(
                    &mut self.current,
                    Value::nil(),
                )))
            }
            PredicateMode::Sole { found } if matches => {
                if found.is_some() {
                    return Err(SemaError::eval(format!(
                        "{}: more than one matching item",
                        self.hof
                    )));
                }
                *found = Some(self.current.clone());
            }
            PredicateMode::Any
            | PredicateMode::Every
            | PredicateMode::DropWhile
            | PredicateMode::Find
            | PredicateMode::Sole { .. } => {}
        }

        self.continue_or_finish()
    }
}

/// Start a cooperative predicate scan, or produce the mode's empty-input value
/// without invoking the predicate.
fn predicate_call(
    predicate: &Value,
    source: &Value,
    mut mode: PredicateMode,
    hof: &'static str,
) -> NativeResult {
    let mut remaining = PredicateItems::from_value(source, hof)?;
    let Some(first) = remaining.pop_front() else {
        return mode.finish(hof).map(NativeOutcome::Return);
    };
    let continuation = Box::new(PredicateContinuation {
        hof,
        predicate: predicate.clone(),
        current: first.clone(),
        remaining,
        mode,
    });
    Ok(NativeOutcome::Call(NativeCall {
        callable: predicate.clone(),
        args: vec![first.clone()],
        continuation,
    }))
}

enum FoldSequenceItems {
    Retained {
        source: Value,
        start: usize,
        end: usize,
    },
    Snapshot {
        items: Vec<Value>,
        start: usize,
        end: usize,
    },
}

impl FoldSequenceItems {
    fn from_value(source: &Value, hof: &'static str) -> Result<Self, SemaError> {
        match get_sequence(source, hof)? {
            Cow::Borrowed(items) => Ok(Self::Retained {
                source: source.clone(),
                start: 0,
                end: items.len(),
            }),
            Cow::Owned(items) => {
                let end = items.len();
                Ok(Self::Snapshot {
                    items,
                    start: 0,
                    end,
                })
            }
        }
    }

    fn pop_front(&mut self) -> Option<Value> {
        match self {
            Self::Retained {
                source, start, end, ..
            } if *start < *end => {
                let item = source
                    .as_list()
                    .or_else(|| source.as_vector())
                    .and_then(|items| items.get(*start))
                    .cloned();
                *start += usize::from(item.is_some());
                item
            }
            Self::Snapshot {
                items, start, end, ..
            } if *start < *end => {
                let item = std::mem::replace(&mut items[*start], Value::nil());
                *start += 1;
                Some(item)
            }
            Self::Retained { .. } | Self::Snapshot { .. } => None,
        }
    }

    fn pop_back(&mut self) -> Option<Value> {
        match self {
            Self::Retained {
                source, start, end, ..
            } if *start < *end => {
                *end -= 1;
                source
                    .as_list()
                    .or_else(|| source.as_vector())
                    .and_then(|items| items.get(*end))
                    .cloned()
            }
            Self::Snapshot {
                items, start, end, ..
            } if *start < *end => {
                *end -= 1;
                Some(std::mem::replace(&mut items[*end], Value::nil()))
            }
            Self::Retained { .. } | Self::Snapshot { .. } => None,
        }
    }

    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        match self {
            Self::Retained { source, .. } => sink(GcEdge::Value(source)),
            Self::Snapshot {
                items, start, end, ..
            } => {
                for item in &items[*start..*end] {
                    sink(GcEdge::Value(item));
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FoldDirection {
    Forward,
    Reverse,
}

enum FoldItems {
    Sequence {
        items: FoldSequenceItems,
        direction: FoldDirection,
    },
    F64Array {
        source: Value,
        next: usize,
    },
    I64Array {
        source: Value,
        next: usize,
    },
}

impl FoldItems {
    fn pop_front(&mut self) -> Option<Value> {
        match self {
            Self::Sequence {
                items,
                direction: FoldDirection::Forward,
            } => items.pop_front(),
            Self::Sequence {
                items,
                direction: FoldDirection::Reverse,
            } => items.pop_back(),
            Self::F64Array { source, next } => {
                let item = source
                    .as_f64_array()
                    .and_then(|items| items.get(*next))
                    .copied()
                    .map(Value::float);
                *next += usize::from(item.is_some());
                item
            }
            Self::I64Array { source, next } => {
                let item = source
                    .as_i64_array()
                    .and_then(|items| items.get(*next))
                    .copied()
                    .map(Value::int);
                *next += usize::from(item.is_some());
                item
            }
        }
    }

    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        match self {
            Self::Sequence { items, .. } => items.trace(sink),
            Self::F64Array { source, .. } | Self::I64Array { source, .. } => {
                sink(GcEdge::Value(source));
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FoldOrder {
    AccumulatorThenItem,
    ItemThenAccumulator,
}

impl FoldOrder {
    fn args(self, accumulator: Value, item: Value) -> Vec<Value> {
        match self {
            Self::AccumulatorThenItem => vec![accumulator, item],
            Self::ItemThenAccumulator => vec![item, accumulator],
        }
    }
}

/// Cooperative fold state. Each combiner result becomes the accumulator
/// argument of the next structural runtime call.
struct FoldContinuation {
    hof: &'static str,
    combiner: Value,
    remaining: FoldItems,
    order: FoldOrder,
}

impl Trace for FoldContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.combiner));
        self.remaining.trace(sink);
        true
    }
}

impl NativeContinuation for FoldContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        // The returned value is the new accumulator.
        let acc = resume_value(input, self.hof)?;
        match self.remaining.pop_front() {
            Some(next) => Ok(NativeOutcome::Call(NativeCall {
                callable: self.combiner.clone(),
                args: self.order.args(acc, next),
                continuation: self,
            })),
            None => Ok(NativeOutcome::Return(acc)),
        }
    }
}

fn start_fold(
    combiner: &Value,
    init: Value,
    mut remaining: FoldItems,
    order: FoldOrder,
    hof: &'static str,
) -> NativeOutcome {
    let Some(first) = remaining.pop_front() else {
        return NativeOutcome::Return(init);
    };
    let continuation = Box::new(FoldContinuation {
        hof,
        combiner: combiner.clone(),
        remaining,
        order,
    });
    NativeOutcome::Call(NativeCall {
        callable: combiner.clone(),
        args: order.args(init, first),
        continuation,
    })
}

fn fold_sequence_call(
    combiner: &Value,
    init: Value,
    source: &Value,
    direction: FoldDirection,
    order: FoldOrder,
    hof: &'static str,
) -> NativeResult {
    let items = FoldSequenceItems::from_value(source, hof)?;
    Ok(start_fold(
        combiner,
        init,
        FoldItems::Sequence { items, direction },
        order,
        hof,
    ))
}

fn reduce_call(combiner: &Value, source: &Value) -> NativeResult {
    let mut items = FoldSequenceItems::from_value(source, "reduce")?;
    let init = items
        .pop_front()
        .ok_or_else(|| SemaError::eval("reduce: empty list"))?;
    Ok(start_fold(
        combiner,
        init,
        FoldItems::Sequence {
            items,
            direction: FoldDirection::Forward,
        },
        FoldOrder::AccumulatorThenItem,
        "reduce",
    ))
}

pub(crate) fn fold_f64_array_call(
    combiner: &Value,
    init: Value,
    source: Value,
    hof: &'static str,
) -> NativeOutcome {
    start_fold(
        combiner,
        init,
        FoldItems::F64Array { source, next: 0 },
        FoldOrder::AccumulatorThenItem,
        hof,
    )
}

pub(crate) fn fold_i64_array_call(
    combiner: &Value,
    init: Value,
    source: Value,
    hof: &'static str,
) -> NativeOutcome {
    start_fold(
        combiner,
        init,
        FoldItems::I64Array { source, next: 0 },
        FoldOrder::AccumulatorThenItem,
        hof,
    )
}

/// Cooperative continuation for `for-each` (Task 04). Runs the callback for each
/// element as a fresh runtime Call for its side effects and discards the result,
/// returning nil once every element has been visited.
struct ForEachContinuation {
    callback: Value,
    remaining: VecDeque<Value>,
}

impl Trace for ForEachContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.callback));
        for item in &self.remaining {
            sink(GcEdge::Value(item));
        }
        true
    }
}

impl NativeContinuation for ForEachContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        // Discard the callback's value; for-each is run for effect.
        resume_value(input, "for-each")?;
        match self.remaining.pop_front() {
            Some(next) => Ok(NativeOutcome::Call(NativeCall {
                callable: self.callback.clone(),
                args: vec![next],
                continuation: self,
            })),
            None => Ok(NativeOutcome::Return(Value::nil())),
        }
    }
}

/// Initial cooperative `NativeOutcome::Call` for `for-each`. Empty input is a
/// no-op, so it returns nil directly.
fn for_each_call(callback: &Value, items: &[Value]) -> NativeOutcome {
    let Some((first, rest)) = items.split_first() else {
        return NativeOutcome::Return(Value::nil());
    };
    let continuation = Box::new(ForEachContinuation {
        callback: callback.clone(),
        remaining: rest.iter().cloned().collect(),
    });
    NativeOutcome::Call(NativeCall {
        callable: callback.clone(),
        args: vec![first.clone()],
        continuation,
    })
}

/// Cooperative continuation for `sort-by` (Task 04). Sorting can't interleave
/// with async work per comparison, so the key function is what runs
/// cooperatively: this collects the key for every element (each via a fresh
/// runtime Call that may park/resume) BEFORE sorting synchronously by key,
/// preserving the legacy stable-by-key order.
struct SortByContinuation {
    key_fn: Value,
    /// The element whose key the next `resume` carries.
    current: Value,
    remaining: VecDeque<Value>,
    keyed: Vec<(Value, Value)>,
}

impl Trace for SortByContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.key_fn));
        sink(GcEdge::Value(&self.current));
        for item in &self.remaining {
            sink(GcEdge::Value(item));
        }
        for (key, item) in &self.keyed {
            sink(GcEdge::Value(key));
            sink(GcEdge::Value(item));
        }
        true
    }
}

impl NativeContinuation for SortByContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let key = resume_value(input, "sort-by")?;
        let item = std::mem::replace(&mut self.current, Value::nil());
        self.keyed.push((key, item));
        match self.remaining.pop_front() {
            Some(next) => {
                self.current = next.clone();
                Ok(NativeOutcome::Call(NativeCall {
                    callable: self.key_fn.clone(),
                    args: vec![next],
                    continuation: self,
                }))
            }
            None => {
                let mut keyed = std::mem::take(&mut self.keyed);
                keyed.sort_by(|(ka, _), (kb, _)| ka.cmp(kb));
                let result: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
                Ok(NativeOutcome::Return(Value::list(result)))
            }
        }
    }
}

/// Initial cooperative `NativeOutcome::Call` for `sort-by`: compute the sort key
/// for each element cooperatively before sorting. Empty input needs no keys, so
/// it returns the empty list directly.
fn sort_by_call(key_fn: &Value, items: &[Value]) -> NativeOutcome {
    let Some((first, rest)) = items.split_first() else {
        return NativeOutcome::Return(Value::list(Vec::new()));
    };
    let continuation = Box::new(SortByContinuation {
        key_fn: key_fn.clone(),
        current: first.clone(),
        remaining: rest.iter().cloned().collect(),
        keyed: Vec::with_capacity(items.len()),
    });
    NativeOutcome::Call(NativeCall {
        callable: key_fn.clone(),
        args: vec![first.clone()],
        continuation,
    })
}

/// Bottom-up stable merge-sort state. A comparator call is always represented
/// by a runtime `NativeCall`; the continuation retains the unfinished runs and
/// resumes the merge after that one comparison settles.
struct SortComparatorContinuation {
    comparator: Value,
    runs: VecDeque<VecDeque<Value>>,
    next_runs: Vec<Vec<Value>>,
    left: VecDeque<Value>,
    right: VecDeque<Value>,
    merged: Vec<Value>,
}

impl Trace for SortComparatorContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.comparator));
        for run in &self.runs {
            for item in run {
                sink(GcEdge::Value(item));
            }
        }
        for run in &self.next_runs {
            for item in run {
                sink(GcEdge::Value(item));
            }
        }
        for item in &self.left {
            sink(GcEdge::Value(item));
        }
        for item in &self.right {
            sink(GcEdge::Value(item));
        }
        for item in &self.merged {
            sink(GcEdge::Value(item));
        }
        true
    }
}

impl SortComparatorContinuation {
    fn issue_comparison(self: Box<Self>) -> NativeOutcome {
        let left = self
            .left
            .front()
            .expect("sort merge has a left item")
            .clone();
        let right = self
            .right
            .front()
            .expect("sort merge has a right item")
            .clone();
        NativeOutcome::Call(NativeCall {
            callable: self.comparator.clone(),
            args: vec![left, right],
            continuation: self,
        })
    }

    fn continue_sort(mut self: Box<Self>) -> NativeOutcome {
        loop {
            if !self.left.is_empty() && !self.right.is_empty() {
                return self.issue_comparison();
            }

            self.merged.extend(self.left.drain(..));
            self.merged.extend(self.right.drain(..));
            if !self.merged.is_empty() {
                self.next_runs.push(std::mem::take(&mut self.merged));
            }

            if self.runs.len() >= 2 {
                self.left = self.runs.pop_front().expect("run count checked");
                self.right = self.runs.pop_front().expect("run count checked");
                self.merged = Vec::with_capacity(self.left.len() + self.right.len());
                continue;
            }

            if let Some(unpaired) = self.runs.pop_front() {
                self.next_runs.push(unpaired.into_iter().collect());
            }
            if self.next_runs.len() == 1 {
                return NativeOutcome::Return(Value::list(
                    self.next_runs.pop().expect("one completed run"),
                ));
            }

            self.runs = std::mem::take(&mut self.next_runs)
                .into_iter()
                .map(VecDeque::from)
                .collect();
        }
    }
}

impl NativeContinuation for SortComparatorContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let ordering = sort_comparator_ordering(&resume_value(input, "sort")?);
        let next = match ordering {
            std::cmp::Ordering::Greater => self
                .right
                .pop_front()
                .expect("active comparison has a right item"),
            std::cmp::Ordering::Less | std::cmp::Ordering::Equal => self
                .left
                .pop_front()
                .expect("active comparison has a left item"),
        };
        self.merged.push(next);
        Ok(self.continue_sort())
    }
}

fn sort_comparator_call(comparator: &Value, items: Vec<Value>) -> NativeOutcome {
    if items.len() < 2 {
        return NativeOutcome::Return(Value::list(items));
    }
    let mut runs: VecDeque<VecDeque<Value>> = items
        .into_iter()
        .map(|item| VecDeque::from([item]))
        .collect();
    let left = runs.pop_front().expect("sort has at least two runs");
    let right = runs.pop_front().expect("sort has at least two runs");
    Box::new(SortComparatorContinuation {
        comparator: comparator.clone(),
        runs,
        next_runs: Vec::new(),
        merged: Vec::with_capacity(left.len() + right.len()),
        left,
        right,
    })
    .issue_comparison()
}

enum KeyProjectionMode {
    GroupBy { groups: Vec<(Value, Vec<Value>)> },
    KeyBy { keyed: BTreeMap<Value, Value> },
}

impl KeyProjectionMode {
    fn accept(&mut self, key: Value, item: Value) {
        match self {
            Self::GroupBy { groups } => {
                if let Some((_, items)) = groups.iter_mut().find(|(existing, _)| existing == &key) {
                    items.push(item);
                } else {
                    groups.push((key, vec![item]));
                }
            }
            Self::KeyBy { keyed } => {
                keyed.insert(key, item);
            }
        }
    }

    fn finish(&mut self) -> Value {
        match self {
            Self::GroupBy { groups } => {
                let mut grouped = BTreeMap::new();
                for (key, items) in std::mem::take(groups) {
                    grouped.insert(key, Value::list(items));
                }
                Value::map(grouped)
            }
            Self::KeyBy { keyed } => Value::map(std::mem::take(keyed)),
        }
    }

    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        match self {
            Self::GroupBy { groups } => {
                for (key, items) in groups {
                    sink(GcEdge::Value(key));
                    for item in items {
                        sink(GcEdge::Value(item));
                    }
                }
            }
            Self::KeyBy { keyed } => {
                for (key, item) in keyed {
                    sink(GcEdge::Value(key));
                    sink(GcEdge::Value(item));
                }
            }
        }
    }
}

struct KeyProjectionContinuation {
    hof: &'static str,
    key_fn: Value,
    current: Value,
    remaining: PredicateItems,
    mode: KeyProjectionMode,
}

impl KeyProjectionContinuation {
    fn continue_or_finish(mut self: Box<Self>) -> NativeResult {
        match self.remaining.pop_front() {
            Some(next) => {
                self.current = next.clone();
                Ok(NativeOutcome::Call(NativeCall {
                    callable: self.key_fn.clone(),
                    args: vec![next],
                    continuation: self,
                }))
            }
            None => Ok(NativeOutcome::Return(self.mode.finish())),
        }
    }
}

impl Trace for KeyProjectionContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.key_fn));
        sink(GcEdge::Value(&self.current));
        self.remaining.trace(sink);
        self.mode.trace(sink);
        true
    }
}

impl NativeContinuation for KeyProjectionContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let key = resume_value(input, self.hof)?;
        let item = std::mem::replace(&mut self.current, Value::nil());
        self.mode.accept(key, item);
        self.continue_or_finish()
    }
}

fn key_projection_call(
    key_fn: &Value,
    source: &Value,
    mut mode: KeyProjectionMode,
    hof: &'static str,
) -> NativeResult {
    let mut remaining = PredicateItems::from_value(source, hof)?;
    let Some(first) = remaining.pop_front() else {
        return Ok(NativeOutcome::Return(mode.finish()));
    };
    Ok(NativeOutcome::Call(NativeCall {
        callable: key_fn.clone(),
        args: vec![first.clone()],
        continuation: Box::new(KeyProjectionContinuation {
            hof,
            key_fn: key_fn.clone(),
            current: first,
            remaining,
            mode,
        }),
    }))
}

#[derive(Clone, Copy)]
pub(crate) enum CollectMode {
    Values,
    FlattenedValues,
    String,
    F64Array,
    I64Array,
}

enum CollectResults {
    Values(Vec<Value>),
    FlattenedValues(Vec<Value>),
    String(String),
    F64Array(Vec<f64>),
    I64Array(Vec<i64>),
}

impl CollectResults {
    fn new(mode: CollectMode, capacity: usize) -> Self {
        match mode {
            CollectMode::Values => Self::Values(Vec::with_capacity(capacity)),
            CollectMode::FlattenedValues => Self::FlattenedValues(Vec::with_capacity(capacity)),
            CollectMode::String => Self::String(String::with_capacity(capacity)),
            CollectMode::F64Array => Self::F64Array(Vec::with_capacity(capacity)),
            CollectMode::I64Array => Self::I64Array(Vec::with_capacity(capacity)),
        }
    }

    fn push(&mut self, value: Value) -> Result<(), SemaError> {
        match self {
            Self::Values(results) => results.push(value),
            Self::FlattenedValues(results) => {
                if let Some(list) = value.as_list() {
                    results.extend(list.iter().cloned());
                } else if let Some(vector) = value.as_vector() {
                    results.extend(vector.iter().cloned());
                } else {
                    results.push(value);
                }
            }
            Self::String(result) => {
                if let Some(character) = value.as_char() {
                    result.push(character);
                } else if let Some(string) = value.as_str() {
                    result.push_str(string);
                } else {
                    return Err(SemaError::type_error("char or string", value.type_name()));
                }
            }
            Self::F64Array(results) => {
                let number = value
                    .as_float()
                    .or_else(|| value.as_int().map(|integer| integer as f64))
                    .ok_or_else(|| {
                        SemaError::type_error(
                            "number (f64-array/map callback must return number)",
                            value.type_name(),
                        )
                    })?;
                results.push(number);
            }
            Self::I64Array(results) => {
                let integer = value.as_int().ok_or_else(|| {
                    SemaError::type_error(
                        "integer (i64-array/map callback must return integer)",
                        value.type_name(),
                    )
                })?;
                results.push(integer);
            }
        }
        Ok(())
    }

    fn finish(self) -> Value {
        match self {
            Self::Values(results) | Self::FlattenedValues(results) => Value::list(results),
            Self::String(result) => Value::string_owned(result),
            Self::F64Array(results) => Value::f64_array(results),
            Self::I64Array(results) => Value::i64_array(results),
        }
    }

    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        match self {
            Self::Values(results) | Self::FlattenedValues(results) => {
                for result in results {
                    sink(GcEdge::Value(result));
                }
            }
            Self::String(_) | Self::F64Array(_) | Self::I64Array(_) => {}
        }
    }
}

/// Cooperative ordered callback collector. Its compact source state constructs
/// one argument vector at a time; the output mode validates and accumulates the
/// result before the next structural runtime call is issued.
struct CollectContinuation {
    hof: &'static str,
    callback: Value,
    remaining: CollectCalls,
    results: CollectResults,
}

enum CollectCalls {
    Unary(VecDeque<Value>),
    Indexed { items: Vec<Value>, next: usize },
    Range { next: usize, end: usize },
    String { source: Value, byte_index: usize },
    F64Array { source: Value, next: usize },
    I64Array { source: Value, next: usize },
    Variadic(VecDeque<Vec<Value>>),
}

impl CollectCalls {
    fn output_capacity_hint(&self) -> usize {
        match self {
            Self::Unary(calls) => calls.len(),
            Self::Indexed { items, next } => items.len().saturating_sub(*next),
            Self::Range { next, end } => end.saturating_sub(*next),
            Self::String { source, byte_index } => source
                .as_str()
                .expect("string collector retains a string")
                .len()
                .saturating_sub(*byte_index),
            Self::F64Array { source, next } => source
                .as_f64_array()
                .expect("f64 collector retains an f64-array")
                .len()
                .saturating_sub(*next),
            Self::I64Array { source, next } => source
                .as_i64_array()
                .expect("i64 collector retains an i64-array")
                .len()
                .saturating_sub(*next),
            Self::Variadic(calls) => calls.len(),
        }
    }

    fn pop_front(&mut self) -> Option<Vec<Value>> {
        match self {
            Self::Unary(calls) => calls.pop_front().map(|value| vec![value]),
            Self::Indexed { items, next } => {
                let item = items.get(*next)?.clone();
                let index = *next;
                *next += 1;
                Some(vec![Value::int(index as i64), item])
            }
            Self::Range { next, end } => {
                if *next == *end {
                    return None;
                }
                let index = *next;
                *next += 1;
                Some(vec![Value::int(index as i64)])
            }
            Self::String { source, byte_index } => {
                let string = source.as_str().expect("string collector retains a string");
                let character = string.get(*byte_index..)?.chars().next()?;
                *byte_index += character.len_utf8();
                Some(vec![Value::char(character)])
            }
            Self::F64Array { source, next } => {
                let array = source
                    .as_f64_array()
                    .expect("f64 collector retains an f64-array");
                let number = *array.get(*next)?;
                *next += 1;
                Some(vec![Value::float(number)])
            }
            Self::I64Array { source, next } => {
                let array = source
                    .as_i64_array()
                    .expect("i64 collector retains an i64-array");
                let integer = *array.get(*next)?;
                *next += 1;
                Some(vec![Value::int(integer)])
            }
            Self::Variadic(calls) => calls.pop_front(),
        }
    }

    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) {
        match self {
            Self::Unary(calls) => {
                for value in calls {
                    sink(GcEdge::Value(value));
                }
            }
            Self::Indexed { items, .. } => {
                for item in items {
                    sink(GcEdge::Value(item));
                }
            }
            Self::Range { .. } => {}
            Self::String { source, .. }
            | Self::F64Array { source, .. }
            | Self::I64Array { source, .. } => sink(GcEdge::Value(source)),
            Self::Variadic(calls) => {
                for args in calls {
                    for argument in args {
                        sink(GcEdge::Value(argument));
                    }
                }
            }
        }
    }
}

impl Trace for CollectContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.callback));
        self.remaining.trace(sink);
        self.results.trace(sink);
        true
    }
}

impl NativeContinuation for CollectContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let value = resume_value(input, self.hof)?;
        self.results.push(value)?;
        match self.remaining.pop_front() {
            Some(next) => Ok(NativeOutcome::Call(NativeCall {
                callable: self.callback.clone(),
                args: next,
                continuation: self,
            })),
            None => {
                let CollectContinuation { results, .. } = *self;
                Ok(NativeOutcome::Return(results.finish()))
            }
        }
    }
}

fn start_collect(
    callback: &Value,
    mut calls: CollectCalls,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    let capacity = calls.output_capacity_hint();
    let Some(first) = calls.pop_front() else {
        return NativeOutcome::Return(CollectResults::new(mode, 0).finish());
    };
    NativeOutcome::Call(NativeCall {
        callable: callback.clone(),
        args: first,
        continuation: Box::new(CollectContinuation {
            hof,
            callback: callback.clone(),
            remaining: calls,
            results: CollectResults::new(mode, capacity),
        }),
    })
}

fn collect_call(
    callback: &Value,
    calls: impl IntoIterator<Item = Vec<Value>>,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    start_collect(
        callback,
        CollectCalls::Variadic(calls.into_iter().collect()),
        mode,
        hof,
    )
}

pub(crate) fn collect_unary_call(
    callback: &Value,
    calls: impl IntoIterator<Item = Value>,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    start_collect(
        callback,
        CollectCalls::Unary(calls.into_iter().collect()),
        mode,
        hof,
    )
}

fn collect_indexed_call(
    callback: &Value,
    items: Vec<Value>,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    start_collect(
        callback,
        CollectCalls::Indexed { items, next: 0 },
        mode,
        hof,
    )
}

fn collect_range_call(
    callback: &Value,
    end: usize,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    start_collect(callback, CollectCalls::Range { next: 0, end }, mode, hof)
}

pub(crate) fn collect_string_call(
    callback: &Value,
    source: Value,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    start_collect(
        callback,
        CollectCalls::String {
            source,
            byte_index: 0,
        },
        mode,
        hof,
    )
}

pub(crate) fn collect_f64_array_call(
    callback: &Value,
    source: Value,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    start_collect(
        callback,
        CollectCalls::F64Array { source, next: 0 },
        mode,
        hof,
    )
}

pub(crate) fn collect_i64_array_call(
    callback: &Value,
    source: Value,
    mode: CollectMode,
    hof: &'static str,
) -> NativeOutcome {
    start_collect(
        callback,
        CollectCalls::I64Array { source, next: 0 },
        mode,
        hof,
    )
}

/// Initial cooperative `NativeOutcome::Call` for a multi-list `map`. Zips the
/// argument lists into per-column arg tuples (shortest list truncates), then
/// drives the columns through the shared collector. Empty (any list empty)
/// returns the empty list directly.
fn map_multi_call(args: &[Value]) -> NativeResult {
    let lists: Vec<Cow<[Value]>> = args[1..]
        .iter()
        .map(|a| get_sequence(a, "map"))
        .collect::<Result<_, _>>()?;
    let min_len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
    let mut columns: VecDeque<Vec<Value>> = VecDeque::with_capacity(min_len);
    for i in 0..min_len {
        columns.push_back(lists.iter().map(|l| l[i].clone()).collect());
    }
    Ok(collect_call(&args[0], columns, CollectMode::Values, "map"))
}

/// Cooperative identity continuation: forwards the callback's result straight
/// through as the native's return value. `apply` uses it directly (its result
/// IS the applied call's result); `call-with-values` uses it for the consumer
/// call. Fail-fast on error/cancellation via `resume_value`.
struct IdentityContinuation {
    hof: &'static str,
}

impl Trace for IdentityContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

/// Cooperative teardown for `tap`: ignore the callback's value and return the
/// exact original value after the callback settles. The original remains a GC
/// root while the callback is parked.
struct TapContinuation {
    original: Value,
}

impl Trace for TapContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.original));
        true
    }
}

impl NativeContinuation for TapContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        resume_value(input, "tap")?;
        Ok(NativeOutcome::Return(self.original.clone()))
    }
}

fn tap_call(args: &[Value]) -> NativeResult {
    check_arity!(args, "tap", 2);
    let original = args[0].clone();
    Ok(NativeOutcome::Call(NativeCall {
        callable: args[1].clone(),
        args: vec![original.clone()],
        continuation: Box::new(TapContinuation { original }),
    }))
}

impl NativeContinuation for IdentityContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        Ok(NativeOutcome::Return(resume_value(input, self.hof)?))
    }
}

/// Initial cooperative `NativeOutcome::Call` for `apply`: collect the flattened
/// arg vector (leading fixed args + the final list spread), then drive the
/// applied function as one runtime Call. Every callable crosses this boundary
/// structurally so dual-ABI natives select their runtime implementation and can
/// suspend just like runtime-only natives and VM closures.
fn apply_call(args: &[Value]) -> NativeResult {
    let func = args[0].clone();
    let last = &args[args.len() - 1];
    let last_items = get_sequence(last, "apply")?;
    let mut all_args: Vec<Value> = args[1..args.len() - 1].to_vec();
    all_args.extend(last_items.iter().cloned());
    Ok(NativeOutcome::Call(NativeCall {
        callable: func,
        args: all_args,
        continuation: Box::new(IdentityContinuation { hof: "apply" }),
    }))
}

/// Cooperative continuation for `call-with-values`: after the (callable)
/// producer settles as the initiating runtime Call, spread its result — a
/// `values` bundle's fields, or a lone value — as the consumer's arguments and
/// drive the consumer as a fresh runtime Call. Running BOTH producer and
/// consumer as cooperative Calls means a runtime op in either (an async closure,
/// or a runtime-only-native consumer like `async/resolved`) suspends cleanly.
struct CallWithValuesContinuation {
    consumer: Value,
}

impl Trace for CallWithValuesContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.consumer));
        true
    }
}

impl NativeContinuation for CallWithValuesContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let produced = resume_value(input, "call-with-values")?;
        let call_args = match produced.as_record() {
            Some(rec) if rec.type_tag == intern(MULTIPLE_VALUES_TAG) => rec.fields.clone(),
            _ => vec![produced],
        };
        Ok(NativeOutcome::Call(NativeCall {
            callable: self.consumer.clone(),
            args: call_args,
            continuation: Box::new(IdentityContinuation {
                hof: "call-with-values",
            }),
        }))
    }
}

/// Shared decode of a cooperative callback resume for the HOF continuations: a
/// returned value flows on; an error / cancellation aborts the whole HOF by
/// propagating so the runtime resumes the parked parent VM with the raised error
/// (catchable by an enclosing try/catch), matching the legacy path's fail-fast.
pub(crate) fn resume_value(input: ResumeInput, hof: &str) -> Result<Value, SemaError> {
    match input {
        ResumeInput::Returned(value) => Ok(value),
        ResumeInput::Failed(error) => Err(error),
        ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
            "{hof} callback was cancelled ({reason:?})"
        ))),
        ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
            "{hof} continuation received an unexpected runtime response"
        ))),
    }
}

/// Record `type_tag` used to bundle zero-or-multiple values produced by `values`
/// and unpacked by `call-with-values`. Chosen to be unlikely to collide with a
/// user `define-record-type` tag; R7RS leaves "multiple values leaking into a
/// single-value context" unspecified, so a user constructing this exact tag via
/// `define-record-type` is already outside spec.
const MULTIPLE_VALUES_TAG: &str = "%multiple-values%";

/// Sort category of a value for the comparator-free `sort`. Every real number
/// (fixnum, bignum, rational, float) shares the `Number` family and compares by
/// numeric value, not by tag — otherwise a bignum (a distinct heap tag from a
/// fixnum, though still just "int") would look like a different type than a
/// fixnum and get rejected as heterogeneous. Complex numbers have no total
/// order, so they stay out of `Number` and are only comparable to each other.
/// Every other type is only comparable to its own kind. `sort` refuses to order
/// values whose categories differ, because `Value`'s cross-type `Ord` falls back
/// to an internal tag order that is arbitrary and never what the caller meant.
#[derive(PartialEq, Eq)]
enum SortCategory {
    Number,
    Other(&'static str),
}

fn sort_category(v: &Value) -> SortCategory {
    if v.as_number().is_some_and(|n| n.is_real()) {
        SortCategory::Number
    } else {
        SortCategory::Other(v.type_name())
    }
}

fn sort_comparator_ordering(value: &Value) -> std::cmp::Ordering {
    if let Some(integer) = value.as_int() {
        integer.cmp(&0)
    } else {
        match value.as_bool() {
            Some(true) => std::cmp::Ordering::Less,
            Some(false) => std::cmp::Ordering::Greater,
            None => std::cmp::Ordering::Equal,
        }
    }
}

fn sort_default(mut items: Vec<Value>) -> Result<Value, SemaError> {
    // Reject heterogeneous input up front: comparing across unrelated types
    // would silently fall back to `Value`'s arbitrary tag order. Pass an
    // explicit comparator (`sort-by` / 2-arg `sort`) to order mixed types.
    if let Some(first) = items.first() {
        let category = sort_category(first);
        if let Some(bad) = items.iter().find(|value| sort_category(value) != category) {
            return Err(
                SemaError::type_error(first.type_name(), bad.type_name()).with_hint(
                    "sort orders one type at a time; use `sort-by` or `(sort xs cmp)` \
                 with a comparator to order mixed types",
                ),
            );
        }
    }
    // All-number lists compare by numeric value. `Value`'s `Ord` orders every
    // int before every float regardless of magnitude; `cmp_real` instead spans
    // the real tower exactly, with NaN values ordered after non-NaN values.
    if matches!(items.first().map(sort_category), Some(SortCategory::Number)) {
        items.sort_by(|a, b| {
            let x = a.as_number().expect("number category checked");
            let y = b.as_number().expect("number category checked");
            x.cmp_real(&y).unwrap_or_else(|| {
                let x_nan = matches!(x, SemaNumber::Real(f) if f.is_nan());
                let y_nan = matches!(y, SemaNumber::Real(f) if f.is_nan());
                match (x_nan, y_nan) {
                    (true, true) => std::cmp::Ordering::Equal,
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    (false, false) => std::cmp::Ordering::Equal,
                }
            })
        });
    } else {
        items.sort();
    }
    Ok(Value::list(items))
}

fn sort_legacy(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "sort", 1..=2);
    let mut items = get_sequence(&args[0], "sort")?.to_vec();
    if args.len() == 1 {
        return sort_default(items);
    }

    let mut error = None;
    items.sort_by(|a, b| {
        if error.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match call_function(&args[1], &[a.clone(), b.clone()]) {
            Ok(value) => sort_comparator_ordering(&value),
            Err(cause) => {
                error = Some(cause);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(error) = error {
        Err(error)
    } else {
        Ok(Value::list(items))
    }
}

fn sort_runtime(args: &[Value]) -> NativeResult {
    check_arity!(args, "sort", 1..=2);
    let items = get_sequence(&args[0], "sort")?.to_vec();
    if args.len() == 1 {
        Ok(NativeOutcome::Return(sort_default(items)?))
    } else {
        Ok(sort_comparator_call(&args[1], items))
    }
}

fn repeat_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "list/repeat", 2);
    let n = args[0].as_index("list/repeat")?;
    let val = args[1].clone();
    Ok(Value::list(vec![val; n]))
}

/// Register a higher-order function whose callback may suspend as a DUAL-ABI
/// native. Under a runtime quantum the VM invokes `runtime`, which returns the
/// initial `NativeOutcome::Call` so the runtime drives the callback cooperatively
/// (an async op inside it parks/resumes). Everywhere else (a bare top-level eval
/// or nested/sync re-entry) the VM runs `legacy`, the synchronous per-element
/// path.
pub(crate) fn register_hof(
    env: &sema_core::Env,
    name: &'static str,
    legacy: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static,
    runtime: impl Fn(&[Value]) -> NativeResult + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            legacy,
            move |_ctx, args| runtime(args),
        )),
    );
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "list", |args| Ok(Value::list(args.to_vec())));

    register_fn(env, "vector", |args| Ok(Value::vector(args.to_vec())));

    register_fn(env, "cons", |args| {
        check_arity!(args, "cons", 2);
        if args[1].is_nil() {
            Ok(Value::list(vec![args[0].clone()]))
        } else if let Some(list) = args[1].as_list() {
            let mut new = vec![args[0].clone()];
            new.extend(list.iter().cloned());
            Ok(Value::list(new))
        } else {
            Ok(Value::list(vec![args[0].clone(), args[1].clone()]))
        }
    });

    register_fn(env, "car", first);
    register_fn(env, "first", first);

    register_fn(env, "cdr", rest);
    register_fn(env, "rest", rest);

    register_fn(env, "length", |args| {
        check_arity!(args, "length", 1);
        if let Some(l) = args[0].as_list() {
            Ok(Value::int(l.len() as i64))
        } else if let Some(v) = args[0].as_vector() {
            Ok(Value::int(v.len() as i64))
        } else if let Some(s) = args[0].as_str() {
            Ok(Value::int(s.chars().count() as i64))
        } else if let Some(m) = args[0].as_map_rc() {
            Ok(Value::int(m.len() as i64))
        } else if let Some(m) = args[0].as_hashmap_rc() {
            Ok(Value::int(m.len() as i64))
        } else if let Some(bv) = args[0].as_bytevector() {
            Ok(Value::int(bv.len() as i64))
        } else if let Some(arr) = args[0].as_f64_array() {
            Ok(Value::int(arr.len() as i64))
        } else if let Some(arr) = args[0].as_i64_array() {
            Ok(Value::int(arr.len() as i64))
        } else if let Some(arr) = args[0].as_mutable_array() {
            Ok(Value::int(arr.items.borrow().len() as i64))
        } else {
            Err(SemaError::type_error(
                "list, vector, string, map, hashmap, bytevector, typed array, or mutable-array",
                args[0].type_name(),
            )
            .with_hint("length: expected a sequence or collection"))
        }
    });

    register_fn(env, "append", |args| {
        let mut result = Vec::new();
        for arg in args {
            if let Some(l) = arg.as_list() {
                result.extend(l.iter().cloned());
            } else if let Some(v) = arg.as_vector() {
                result.extend(v.iter().cloned());
            } else {
                return Err(SemaError::type_error("list or vector", arg.type_name())
                    .with_hint("append: every argument must be a list or vector"));
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "reverse", |args| {
        check_arity!(args, "reverse", 1);
        if let Some(l) = args[0].as_list() {
            let mut v = l.to_vec();
            v.reverse();
            Ok(Value::list(v))
        } else if let Some(v) = args[0].as_vector() {
            let mut items = v.to_vec();
            items.reverse();
            Ok(Value::vector(items))
        } else {
            Err(SemaError::type_error("list or vector", args[0].type_name())
                .with_hint("reverse: argument 1 must be a list or vector"))
        }
    });

    register_fn(env, "nth", |args| {
        check_arity!(args, "nth", 2);
        let idx_i = args[1].as_int().ok_or_else(|| {
            // A collection in the index slot almost always means swapped args.
            let swapped = args[1].as_list().is_some() || args[1].as_vector().is_some();
            let hint = if swapped {
                "nth: argument order is (nth collection index) — looks like the arguments are swapped"
            } else {
                "nth: argument order is (nth collection index); the index must be an integer"
            };
            SemaError::type_error("int", args[1].type_name()).with_hint(hint)
        })?;
        if idx_i < 0 {
            return Err(
                SemaError::eval(format!("nth: index must be non-negative, got {idx_i}"))
                    .with_hint("indices are 0-based; use (last xs) for the last element"),
            );
        }
        let idx = idx_i as usize;
        if let Some(l) = args[0].as_list() {
            l.get(idx).cloned().ok_or_else(|| {
                SemaError::eval(format!("index {idx} out of bounds (length {})", l.len()))
            })
        } else if let Some(v) = args[0].as_vector() {
            v.get(idx).cloned().ok_or_else(|| {
                SemaError::eval(format!("index {idx} out of bounds (length {})", v.len()))
            })
        } else if let Some(arr) = args[0].as_mutable_array() {
            let items = arr.items.borrow();
            items.get(idx).cloned().ok_or_else(|| {
                SemaError::eval(format!(
                    "index {idx} out of bounds (length {})",
                    items.len()
                ))
            })
        } else {
            Err(SemaError::type_error("list or vector", args[0].type_name())
                .with_hint("nth: argument 1 must be a list, vector, or mutable-array"))
        }
    });

    // `map` drives its callback COOPERATIVELY under the runtime (its `runtime`
    // ABI returns the initial `NativeOutcome::Call`) so an async op inside the
    // callback (spawn/await/channel) parks and resumes correctly. Both the
    // single-list and multi-list (zipped) shapes are cooperative; the legacy
    // value ABI keeps the synchronous per-element path for bare/top-level eval.
    register_hof(
        env,
        "map",
        |args| {
            check_arity!(args, "map", 2..);
            if args.len() == 2 {
                let items = get_sequence(&args[1], "map")?;
                let mut result = Vec::with_capacity(items.len());
                for item in items.iter() {
                    result.push(call_function(&args[0], &[item.clone()])?);
                }
                Ok(Value::list(result))
            } else {
                map_multi(args)
            }
        },
        |args| {
            check_arity!(args, "map", 2..);
            if args.len() == 2 {
                let items = get_sequence(&args[1], "map")?;
                Ok(map_call(&args[0], &items))
            } else {
                map_multi_call(args)
            }
        },
    );

    register_hof(
        env,
        "map-indexed",
        |args| {
            check_arity!(args, "map-indexed", 2);
            let items = get_sequence(&args[1], "map-indexed")?;
            let mut result = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                result.push(call_function(
                    &args[0],
                    &[Value::int(i as i64), item.clone()],
                )?);
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "map-indexed", 2);
            let items = get_sequence(&args[1], "map-indexed")?;
            Ok(collect_indexed_call(
                &args[0],
                items.to_vec(),
                CollectMode::Values,
                "map-indexed",
            ))
        },
    );

    register_fn(env, "enumerate", |args| {
        check_arity!(args, "enumerate", 1);
        let items = get_sequence(&args[0], "enumerate")?;
        let mut result = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            result.push(Value::list(vec![Value::int(i as i64), item.clone()]));
        }
        Ok(Value::list(result))
    });

    // `filter` drives its predicate COOPERATIVELY under the runtime (see `map`).
    register_hof(
        env,
        "filter",
        |args| {
            check_arity!(args, "filter", 2);
            let items = get_sequence(&args[1], "filter")?;
            let mut result = Vec::new();
            for item in items.iter() {
                let owned = item.clone();
                let keep = call_function(&args[0], std::slice::from_ref(&owned))?;
                if keep.is_truthy() {
                    result.push(owned);
                }
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "filter", 2);
            predicate_call(
                &args[0],
                &args[1],
                PredicateMode::Select {
                    keep_when_truthy: true,
                    results: Vec::new(),
                },
                "filter",
            )
        },
    );

    // `foldl` threads its accumulator COOPERATIVELY under the runtime (see `map`).
    register_hof(
        env,
        "foldl",
        |args| {
            check_arity!(args, "foldl", 3);
            let items = get_sequence(&args[2], "foldl")?;
            let mut acc = args[1].clone();
            for item in items.iter() {
                // Owned handoff: the accumulator moves into the callback frame so
                // uniqueness-gated in-place updates (assoc & co.) can fire.
                let mut cb_args = [std::mem::replace(&mut acc, Value::nil()), item.clone()];
                acc = call_function_owned(&args[0], &mut cb_args)?;
            }
            Ok(acc)
        },
        |args| {
            check_arity!(args, "foldl", 3);
            fold_sequence_call(
                &args[0],
                args[1].clone(),
                &args[2],
                FoldDirection::Forward,
                FoldOrder::AccumulatorThenItem,
                "foldl",
            )
        },
    );

    // `for-each` runs its callback COOPERATIVELY under the runtime (see `map`).
    register_hof(
        env,
        "for-each",
        |args| {
            check_arity!(args, "for-each", 2);
            let items = get_sequence(&args[1], "for-each")?;
            for item in items.iter() {
                call_function(&args[0], &[item.clone()])?;
            }
            Ok(Value::nil())
        },
        |args| {
            check_arity!(args, "for-each", 2);
            let items = get_sequence(&args[1], "for-each")?;
            Ok(for_each_call(&args[0], &items))
        },
    );

    register_fn(env, "range", |args| {
        check_arity!(args, "range", 1..=3);
        let (start, end, step) = match args.len() {
            1 => (
                0i64,
                args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?,
                1i64,
            ),
            2 => {
                let s = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let e = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                (s, e, 1)
            }
            _ => {
                let s = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let e = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                let st = args[2]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?;
                (s, e, st)
            }
        };
        if step == 0 {
            return Err(SemaError::eval("range: step cannot be 0"));
        }
        let mut result = Vec::new();
        let mut i = start;
        if step > 0 {
            while i < end {
                result.push(Value::int(i));
                i += step;
            }
        } else {
            while i > end {
                result.push(Value::int(i));
                i += step;
            }
        }
        Ok(Value::list(result))
    });

    // Under the runtime, `apply` always hands its callable to the structural
    // `NativeOutcome::Call` ABI. Besides runtime-only natives and VM closures,
    // this is required for dual-ABI natives: invoking their value callback here
    // would bypass `invoke_runtime` and turn a legal suspension into a legacy
    // synchronous-callback error. The legacy value ABI remains synchronous for
    // host-only calls where no runtime can drive a suspension.
    register_hof(
        env,
        "apply",
        |args| {
            check_arity!(args, "apply", 2..);
            let func = &args[0];
            if is_runtime_only_native(func) {
                return Err(runtime_only_sync_apply_err(func, "apply"));
            }
            // Last arg must be a list, preceding args are prepended
            let last = &args[args.len() - 1];
            let last_items = get_sequence(last, "apply")?;
            let mut all_args: Vec<Value> = args[1..args.len() - 1].to_vec();
            all_args.extend(last_items.iter().cloned());
            call_function(func, &all_args)
        },
        |args| {
            check_arity!(args, "apply", 2..);
            apply_call(args)
        },
    );

    // R7RS `values`: 1 arg is just that value (so it flows through ordinary
    // single-value contexts like `(+ 1 (values 2))`); 0 or 2+ args bundle into
    // a `Record` tagged `MULTIPLE_VALUES_TAG` that only `call-with-values`
    // inspects. Any other consumer sees an opaque record — R7RS leaves
    // multi-values-in-single-value-context unspecified.
    register_fn(env, "values", |args| match args.len() {
        1 => Ok(args[0].clone()),
        _ => Ok(Value::record(Record {
            type_tag: intern(MULTIPLE_VALUES_TAG),
            field_names: vec![],
            fields: args.to_vec(),
        })),
    });

    // R7RS `call-with-values`: call `producer` with no args, spread its result
    // (a values-bundle or a single ordinary value) as arguments to `consumer`.
    // COOPERATIVE under the runtime: a CALLABLE producer runs as the initiating
    // Call and `CallWithValuesContinuation` then drives the consumer as a fresh
    // Call, so a runtime op in EITHER the producer or the consumer (an async
    // closure, or a runtime-only-native consumer like `async/resolved`) suspends
    // cleanly. A NON-callable producer skips the cooperative path and runs
    // through `call_function`, preserving its exact "not callable" error. The
    // legacy value ABI keeps the fully synchronous path for a bare / top-level
    // eval.
    register_hof(
        env,
        "call-with-values",
        |args| {
            check_arity!(args, "call-with-values", 2);
            if is_runtime_only_native(&args[0]) {
                return Err(runtime_only_sync_apply_err(&args[0], "call-with-values"));
            }
            let produced = call_function(&args[0], &[])?;
            if is_runtime_only_native(&args[1]) {
                return Err(runtime_only_sync_apply_err(&args[1], "call-with-values"));
            }
            match produced.as_record() {
                Some(rec) if rec.type_tag == intern(MULTIPLE_VALUES_TAG) => {
                    call_function(&args[1], &rec.fields.clone())
                }
                _ => call_function(&args[1], &[produced]),
            }
        },
        |args| {
            check_arity!(args, "call-with-values", 2);
            if is_callable(&args[0]) {
                Ok(NativeOutcome::Call(NativeCall {
                    callable: args[0].clone(),
                    args: Vec::new(),
                    continuation: Box::new(CallWithValuesContinuation {
                        consumer: args[1].clone(),
                    }),
                }))
            } else {
                // Non-callable producer: raise the evaluator's exact "not
                // callable" error directly. We can't route through
                // `call_function` here — its host-only `with_stdlib_ctx` seam
                // panics inside a runtime quantum — nor through a
                // `NativeOutcome::Call`, whose non-callable arm surfaces a
                // different ("expected callable") message.
                Err(
                    SemaError::eval(format!(
                        "not callable: {} ({})",
                        args[0],
                        args[0].type_name()
                    ))
                    .with_hint("expected a function, lambda, or keyword"),
                )
            }
        },
    );

    register_fn(env, "take", |args| {
        check_arity!(args, "take", 2);
        let n = args[0].as_index("take")?;
        let items = get_sequence(&args[1], "take")?;
        let end = n.min(items.len());
        Ok(Value::list(items[..end].to_vec()))
    });

    register_fn(env, "drop", |args| {
        check_arity!(args, "drop", 2);
        let n = args[0].as_index("drop")?;
        let items = get_sequence(&args[1], "drop")?;
        let start = n.min(items.len());
        Ok(Value::list(items[start..].to_vec()))
    });

    register_fn(env, "last", |args| {
        check_arity!(args, "last", 1);
        let items = get_sequence(&args[0], "last")?;
        Ok(items.last().cloned().unwrap_or(Value::nil()))
    });

    register_fn(env, "zip", |args| {
        check_arity!(args, "zip", 2..);
        let lists: Vec<Cow<[Value]>> = args
            .iter()
            .map(|a| get_sequence(a, "zip"))
            .collect::<Result<_, _>>()?;
        let min_len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
        let mut result = Vec::with_capacity(min_len);
        for i in 0..min_len {
            let tuple: Vec<Value> = lists.iter().map(|l| l[i].clone()).collect();
            result.push(Value::list(tuple));
        }
        Ok(Value::list(result))
    });

    register_fn(env, "flatten", |args| {
        check_arity!(args, "flatten", 1);
        let items = get_sequence(&args[0], "flatten")?;
        let mut result = Vec::new();
        for item in items.iter() {
            if let Some(l) = item.as_list() {
                result.extend(l.iter().cloned());
            } else if let Some(v) = item.as_vector() {
                result.extend(v.iter().cloned());
            } else {
                result.push(item.clone());
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "member", |args| {
        check_arity!(args, "member", 2);
        let items = get_sequence(&args[1], "member")?;
        for (i, item) in items.iter().enumerate() {
            if item == &args[0] {
                return Ok(Value::list(items[i..].to_vec()));
            }
        }
        Ok(Value::bool(false))
    });

    register_hof(
        env,
        "any",
        |args| {
            check_arity!(args, "any", 2);
            let items = get_sequence(&args[1], "any")?;
            for item in items.iter() {
                if call_function(&args[0], &[item.clone()])?.is_truthy() {
                    return Ok(Value::bool(true));
                }
            }
            Ok(Value::bool(false))
        },
        |args| {
            check_arity!(args, "any", 2);
            predicate_call(&args[0], &args[1], PredicateMode::Any, "any")
        },
    );

    register_hof(
        env,
        "every",
        |args| {
            check_arity!(args, "every", 2);
            let items = get_sequence(&args[1], "every")?;
            for item in items.iter() {
                if !call_function(&args[0], &[item.clone()])?.is_truthy() {
                    return Ok(Value::bool(false));
                }
            }
            Ok(Value::bool(true))
        },
        |args| {
            check_arity!(args, "every", 2);
            predicate_call(&args[0], &args[1], PredicateMode::Every, "every")
        },
    );
    // Note: canonical predicate-? aliases (`any?`, `every?`) are registered
    // at the end of this fn (see below).

    // `reduce` threads its accumulator COOPERATIVELY under the runtime (see
    // `foldl`): seed with the first element and fold the rest.
    register_hof(
        env,
        "reduce",
        |args| {
            check_arity!(args, "reduce", 2);
            let items = get_sequence(&args[1], "reduce")?;
            if items.is_empty() {
                return Err(SemaError::eval("reduce: empty list"));
            }
            let mut acc = items[0].clone();
            for item in &items[1..] {
                // Owned handoff — see foldl.
                let mut cb_args = [std::mem::replace(&mut acc, Value::nil()), item.clone()];
                acc = call_function_owned(&args[0], &mut cb_args)?;
            }
            Ok(acc)
        },
        |args| {
            check_arity!(args, "reduce", 2);
            reduce_call(&args[0], &args[1])
        },
    );

    register_hof(
        env,
        "partition",
        |args| {
            check_arity!(args, "partition", 2);
            let items = get_sequence(&args[1], "partition")?;
            let mut matching = Vec::new();
            let mut non_matching = Vec::new();
            for item in items.iter() {
                if call_function(&args[0], &[item.clone()])?.is_truthy() {
                    matching.push(item.clone());
                } else {
                    non_matching.push(item.clone());
                }
            }
            Ok(Value::list(vec![
                Value::list(matching),
                Value::list(non_matching),
            ]))
        },
        |args| {
            check_arity!(args, "partition", 2);
            predicate_call(
                &args[0],
                &args[1],
                PredicateMode::Partition {
                    matching: Vec::new(),
                    non_matching: Vec::new(),
                },
                "partition",
            )
        },
    );

    register_hof(
        env,
        "foldr",
        |args| {
            check_arity!(args, "foldr", 3);
            let items = get_sequence(&args[2], "foldr")?;
            let mut acc = args[1].clone();
            for item in items.iter().rev() {
                acc = call_function(&args[0], &[item.clone(), acc])?;
            }
            Ok(acc)
        },
        |args| {
            check_arity!(args, "foldr", 3);
            fold_sequence_call(
                &args[0],
                args[1].clone(),
                &args[2],
                FoldDirection::Reverse,
                FoldOrder::ItemThenAccumulator,
                "foldr",
            )
        },
    );

    // Comparator-driven sorting uses a resumable stable merge under the runtime:
    // each comparison is one structural call, while comparator-free sorting stays
    // synchronous. The plain value ABI retains the host/synchronous implementation.
    register_hof(env, "sort", sort_legacy, sort_runtime);

    register_fn(env, "list/index-of", |args| {
        check_arity!(args, "list/index-of", 2);
        let items = get_sequence(&args[0], "list/index-of")?;
        for (i, item) in items.iter().enumerate() {
            if item == &args[1] {
                return Ok(Value::int(i as i64));
            }
        }
        Ok(Value::nil())
    });

    // Boolean membership — unlike `member` (which returns the Scheme tail-or-#f), this
    // reads as a predicate and allocates nothing.
    register_fn(env, "list/contains?", |args| {
        check_arity!(args, "list/contains?", 2);
        let items = get_sequence(&args[0], "list/contains?")?;
        Ok(Value::bool(items.iter().any(|item| item == &args[1])))
    });

    // Safe indexed access: returns `default` instead of erroring when out of bounds.
    register_fn(env, "list/nth-or", |args| {
        check_arity!(args, "list/nth-or", 3);
        let items = get_sequence(&args[0], "list/nth-or")?;
        let idx = args[1].as_index("list/nth-or")?;
        Ok(items.get(idx).cloned().unwrap_or_else(|| args[2].clone()))
    });

    // The last `n` elements (inverse of `take`). Clamps to the sequence length.
    register_fn(env, "list/take-last", |args| {
        check_arity!(args, "list/take-last", 2);
        let n = args[0].as_index("list/take-last")?;
        let items = get_sequence(&args[1], "list/take-last")?;
        let start = items.len().saturating_sub(n);
        Ok(Value::list(items[start..].to_vec()))
    });

    // All but the last `n` elements (drop from the tail). Clamps to empty.
    register_fn(env, "list/drop-last", |args| {
        check_arity!(args, "list/drop-last", 2);
        let n = args[0].as_index("list/drop-last")?;
        let items = get_sequence(&args[1], "list/drop-last")?;
        let end = items.len().saturating_sub(n);
        Ok(Value::list(items[..end].to_vec()))
    });

    register_fn(env, "list/unique", |args| {
        check_arity!(args, "list/unique", 1);
        let items = get_sequence(&args[0], "list/unique")?;
        let mut seen: std::collections::BTreeSet<Value> = std::collections::BTreeSet::new();
        let mut result = Vec::new();
        for item in items.iter() {
            if seen.insert(item.clone()) {
                result.push(item.clone());
            }
        }
        Ok(Value::list(result))
    });

    register_hof(
        env,
        "list/group-by",
        |args| {
            check_arity!(args, "list/group-by", 2);
            let items = get_sequence(&args[1], "list/group-by")?;
            let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
            for item in items.iter() {
                let key = call_function(&args[0], &[item.clone()])?;
                if let Some(group) = groups.iter_mut().find(|(k, _)| k == &key) {
                    group.1.push(item.clone());
                } else {
                    groups.push((key, vec![item.clone()]));
                }
            }
            let mut map = BTreeMap::new();
            for (key, vals) in groups {
                map.insert(key, Value::list(vals));
            }
            Ok(Value::map(map))
        },
        |args| {
            check_arity!(args, "list/group-by", 2);
            key_projection_call(
                &args[0],
                &args[1],
                KeyProjectionMode::GroupBy { groups: Vec::new() },
                "list/group-by",
            )
        },
    );

    register_fn(env, "list/interleave", |args| {
        check_arity!(args, "list/interleave", 2..);
        let lists: Vec<Cow<[Value]>> = args
            .iter()
            .map(|a| get_sequence(a, "list/interleave"))
            .collect::<Result<_, _>>()?;
        let min_len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
        let mut result = Vec::with_capacity(min_len * lists.len());
        for i in 0..min_len {
            for list in &lists {
                result.push(list[i].clone());
            }
        }
        Ok(Value::list(result))
    });

    // `sort-by` computes each element's sort key COOPERATIVELY under the runtime
    // (each key call may park/resume) BEFORE sorting; the sort stays synchronous.
    register_hof(
        env,
        "sort-by",
        |args| {
            check_arity!(args, "sort-by", 2);
            let items = get_sequence(&args[1], "sort-by")?;
            let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(items.len());
            for item in items.iter() {
                let key = call_function(&args[0], &[item.clone()])?;
                keyed.push((key, item.clone()));
            }
            keyed.sort_by(|(ka, _), (kb, _)| ka.cmp(kb));
            let result: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "sort-by", 2);
            let items = get_sequence(&args[1], "sort-by")?;
            Ok(sort_by_call(&args[0], &items))
        },
    );

    register_fn(env, "flatten-deep", |args| {
        check_arity!(args, "flatten-deep", 1);
        let mut out = Vec::new();
        flatten_recursive(&args[0], &mut out);
        Ok(Value::list(out))
    });

    register_fn(env, "interpose", |args| {
        check_arity!(args, "interpose", 2);
        let items = get_sequence(&args[1], "interpose")?;
        if items.is_empty() {
            return Ok(Value::list(vec![]));
        }
        let mut result = Vec::with_capacity(items.len() * 2 - 1);
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                result.push(args[0].clone());
            }
            result.push(item.clone());
        }
        Ok(Value::list(result))
    });

    register_fn(env, "frequencies", |args| {
        check_arity!(args, "frequencies", 1);
        let items = get_sequence(&args[0], "frequencies")?;
        let mut counts: std::collections::BTreeMap<Value, i64> = std::collections::BTreeMap::new();
        for item in items.iter() {
            *counts.entry(item.clone()).or_insert(0) += 1;
        }
        let map: std::collections::BTreeMap<Value, Value> = counts
            .into_iter()
            .map(|(k, v)| (k, Value::int(v)))
            .collect();
        Ok(Value::map(map))
    });

    register_fn(env, "list->vector", |args| {
        check_arity!(args, "list->vector", 1);
        if let Some(l) = args[0].as_list() {
            Ok(Value::vector(l.to_vec()))
        } else {
            Err(SemaError::type_error("list", args[0].type_name())
                .with_hint("list->vector: argument 1 must be a list"))
        }
    });

    register_fn(env, "vector->list", |args| {
        check_arity!(args, "vector->list", 1);
        if let Some(v) = args[0].as_vector() {
            Ok(Value::list(v.to_vec()))
        } else {
            Err(SemaError::type_error("vector", args[0].type_name())
                .with_hint("vector->list: argument 1 must be a vector"))
        }
    });

    register_fn(env, "list/chunk", |args| {
        check_arity!(args, "list/chunk", 2);
        let n = args[0].as_index("list/chunk")?;
        if n == 0 {
            return Err(SemaError::eval("list/chunk: chunk size must be positive"));
        }
        let items = get_sequence(&args[1], "list/chunk")?;
        let mut result = Vec::new();
        for chunk in items.chunks(n) {
            result.push(Value::list(chunk.to_vec()));
        }
        Ok(Value::list(result))
    });

    register_hof(
        env,
        "take-while",
        |args| {
            check_arity!(args, "take-while", 2);
            let items = get_sequence(&args[1], "take-while")?;
            let mut result = Vec::new();
            for item in items.iter() {
                if call_function(&args[0], &[item.clone()])?.is_truthy() {
                    result.push(item.clone());
                } else {
                    break;
                }
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "take-while", 2);
            predicate_call(
                &args[0],
                &args[1],
                PredicateMode::TakeWhile {
                    results: Vec::new(),
                },
                "take-while",
            )
        },
    );

    register_hof(
        env,
        "drop-while",
        |args| {
            check_arity!(args, "drop-while", 2);
            let items = get_sequence(&args[1], "drop-while")?;
            let mut dropping = true;
            let mut result = Vec::new();
            for item in items.iter() {
                if dropping && call_function(&args[0], &[item.clone()])?.is_truthy() {
                    continue;
                }
                dropping = false;
                result.push(item.clone());
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "drop-while", 2);
            predicate_call(&args[0], &args[1], PredicateMode::DropWhile, "drop-while")
        },
    );

    register_fn(env, "list/dedupe", |args| {
        check_arity!(args, "list/dedupe", 1);
        let items = get_sequence(&args[0], "list/dedupe")?;
        let mut result = Vec::new();
        for item in items.iter() {
            if result.last() != Some(item) {
                result.push(item.clone());
            }
        }
        Ok(Value::list(result))
    });

    register_hof(
        env,
        "flat-map",
        |args| {
            check_arity!(args, "flat-map", 2);
            let items = get_sequence(&args[1], "flat-map")?;
            let mut result = Vec::new();
            for item in items.iter() {
                let mapped = call_function(&args[0], &[item.clone()])?;
                if let Some(l) = mapped.as_list() {
                    result.extend(l.iter().cloned());
                } else if let Some(v) = mapped.as_vector() {
                    result.extend(v.iter().cloned());
                } else {
                    result.push(mapped);
                }
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "flat-map", 2);
            let items = get_sequence(&args[1], "flat-map")?;
            Ok(collect_unary_call(
                &args[0],
                items.iter().cloned(),
                CollectMode::FlattenedValues,
                "flat-map",
            ))
        },
    );

    register_fn(env, "list/shuffle", |args| {
        check_arity!(args, "list/shuffle", 1);
        let mut items = get_sequence(&args[0], "list/shuffle")?.to_vec();
        use rand::seq::SliceRandom;
        items.shuffle(&mut rand::rng());
        Ok(Value::list(items))
    });

    register_fn(env, "list/split-at", |args| {
        check_arity!(args, "list/split-at", 2);
        let items = get_sequence(&args[0], "list/split-at")?;
        let n = args[1].as_index("list/split-at")?;
        let n = n.min(items.len());
        let left = items[..n].to_vec();
        let right = items[n..].to_vec();
        Ok(Value::list(vec![Value::list(left), Value::list(right)]))
    });

    register_hof(
        env,
        "list/take-while",
        |args| {
            check_arity!(args, "list/take-while", 2);
            let items = get_sequence(&args[1], "list/take-while")?;
            let mut result = Vec::new();
            for item in items.iter() {
                let keep = call_function(&args[0], &[item.clone()])?;
                if keep.is_truthy() {
                    result.push(item.clone());
                } else {
                    break;
                }
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "list/take-while", 2);
            predicate_call(
                &args[0],
                &args[1],
                PredicateMode::TakeWhile {
                    results: Vec::new(),
                },
                "list/take-while",
            )
        },
    );

    register_hof(
        env,
        "list/drop-while",
        |args| {
            check_arity!(args, "list/drop-while", 2);
            let items = get_sequence(&args[1], "list/drop-while")?;
            let mut dropping = true;
            let mut result = Vec::new();
            for item in items.iter() {
                if dropping {
                    let drop = call_function(&args[0], &[item.clone()])?;
                    if drop.is_truthy() {
                        continue;
                    }
                    dropping = false;
                }
                result.push(item.clone());
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "list/drop-while", 2);
            predicate_call(
                &args[0],
                &args[1],
                PredicateMode::DropWhile,
                "list/drop-while",
            )
        },
    );

    register_fn(env, "list/sum", |args| {
        check_arity!(args, "list/sum", 1);
        let items = get_sequence(&args[0], "list/sum")?;
        let mut int_sum: i64 = 0;
        let mut has_float = false;
        let mut float_sum: f64 = 0.0;
        for item in items.iter() {
            if let Some(n) = item.as_int() {
                int_sum += n;
                float_sum += n as f64;
            } else if let Some(f) = item.as_float() {
                has_float = true;
                float_sum += f;
            } else {
                return Err(SemaError::type_error("number", item.type_name()));
            }
        }
        if has_float {
            Ok(Value::float(float_sum))
        } else {
            Ok(Value::int(int_sum))
        }
    });

    register_fn(env, "list/min", |args| {
        check_arity!(args, "list/min", 1);
        let items = get_sequence(&args[0], "list/min")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/min: empty list"));
        }
        let mut result = items[0].clone();
        for item in &items[1..] {
            if num_lt(item, &result)? {
                result = item.clone();
            }
        }
        Ok(result)
    });

    register_fn(env, "list/max", |args| {
        check_arity!(args, "list/max", 1);
        let items = get_sequence(&args[0], "list/max")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/max: empty list"));
        }
        let mut result = items[0].clone();
        for item in &items[1..] {
            if num_lt(&result, item)? {
                result = item.clone();
            }
        }
        Ok(result)
    });

    register_fn(env, "list/pick", |args| {
        check_arity!(args, "list/pick", 1);
        let items = get_sequence(&args[0], "list/pick")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/pick: empty list"));
        }
        use rand::seq::IndexedRandom;
        let chosen = items.choose(&mut rand::rng()).unwrap();
        Ok(chosen.clone())
    });

    register_fn(env, "list/repeat", repeat_impl);
    register_fn(env, "make-list", repeat_impl);

    register_fn(env, "iota", |args| {
        check_arity!(args, "iota", 1..=3);
        let (count, start, step) = match args.len() {
            1 => {
                let c = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                (c, 0i64, 1i64)
            }
            2 => {
                let c = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let s = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                (c, s, 1)
            }
            _ => {
                let c = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let s = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                let st = args[2]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?;
                (c, s, st)
            }
        };
        let mut result = Vec::with_capacity(count.max(0) as usize);
        let mut val = start;
        for _ in 0..count {
            result.push(Value::int(val));
            val += step;
        }
        Ok(Value::list(result))
    });

    // list/reject — inverse of filter
    register_hof(
        env,
        "list/reject",
        |args| {
            check_arity!(args, "list/reject", 2);
            let items = get_sequence(&args[1], "list/reject")?;
            let mut result = Vec::new();
            for item in items.iter() {
                let reject = call_function(&args[0], &[item.clone()])?;
                if !reject.is_truthy() {
                    result.push(item.clone());
                }
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "list/reject", 2);
            predicate_call(
                &args[0],
                &args[1],
                PredicateMode::Select {
                    keep_when_truthy: false,
                    results: Vec::new(),
                },
                "list/reject",
            )
        },
    );

    // list/pluck — extract a field from list of maps
    register_fn(env, "list/pluck", |args| {
        check_arity!(args, "list/pluck", 2);
        let key = &args[0];
        let items = get_sequence(&args[1], "list/pluck")?;
        let mut result = Vec::with_capacity(items.len());
        for item in items.iter() {
            let val = match item.view_ref() {
                ValueViewRef::Map(m) => m.get(key).cloned().unwrap_or(Value::nil()),
                ValueViewRef::HashMap(m) => m.get(key).cloned().unwrap_or(Value::nil()),
                _ => Value::nil(),
            };
            result.push(val);
        }
        Ok(Value::list(result))
    });

    // list/avg — average of numeric list
    register_fn(env, "list/avg", |args| {
        check_arity!(args, "list/avg", 1);
        let items = get_sequence(&args[0], "list/avg")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/avg: empty list"));
        }
        let mut sum: f64 = 0.0;
        for item in items.iter() {
            if let Some(n) = item.as_int() {
                sum += n as f64;
            } else if let Some(f) = item.as_float() {
                sum += f;
            } else {
                return Err(SemaError::type_error("number", item.type_name()));
            }
        }
        Ok(Value::float(sum / items.len() as f64))
    });

    // list/median — statistical median
    register_fn(env, "list/median", |args| {
        check_arity!(args, "list/median", 1);
        let items = get_sequence(&args[0], "list/median")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/median: empty list"));
        }
        let mut nums: Vec<f64> = Vec::with_capacity(items.len());
        for item in items.iter() {
            if let Some(n) = item.as_int() {
                nums.push(n as f64);
            } else if let Some(f) = item.as_float() {
                nums.push(f);
            } else {
                return Err(SemaError::type_error("number", item.type_name()));
            }
        }
        nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = nums.len() / 2;
        if nums.len().is_multiple_of(2) {
            Ok(Value::float((nums[mid - 1] + nums[mid]) / 2.0))
        } else {
            Ok(Value::float(nums[mid]))
        }
    });

    // list/mode — statistical mode (most frequent)
    register_fn(env, "list/mode", |args| {
        check_arity!(args, "list/mode", 1);
        let items = get_sequence(&args[0], "list/mode")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/mode: empty list"));
        }
        let mut counts: std::collections::BTreeMap<Value, usize> =
            std::collections::BTreeMap::new();
        for item in items.iter() {
            *counts.entry(item.clone()).or_insert(0) += 1;
        }
        let max_count = counts.values().copied().max().unwrap();
        let modes: Vec<Value> = counts
            .into_iter()
            .filter(|(_, c)| *c == max_count)
            .map(|(v, _)| v)
            .collect();
        if modes.len() == 1 {
            Ok(modes.into_iter().next().unwrap())
        } else {
            Ok(Value::list(modes))
        }
    });

    // list/diff — set difference
    register_fn(env, "list/diff", |args| {
        check_arity!(args, "list/diff", 2);
        let a = get_sequence(&args[0], "list/diff")?;
        let b = get_sequence(&args[1], "list/diff")?;
        let b_set: std::collections::BTreeSet<Value> = b.iter().cloned().collect();
        let result: Vec<Value> = a
            .iter()
            .filter(|item| !b_set.contains(item))
            .cloned()
            .collect();
        Ok(Value::list(result))
    });

    // list/intersect — set intersection
    register_fn(env, "list/intersect", |args| {
        check_arity!(args, "list/intersect", 2);
        let a = get_sequence(&args[0], "list/intersect")?;
        let b = get_sequence(&args[1], "list/intersect")?;
        let b_set: std::collections::BTreeSet<Value> = b.iter().cloned().collect();
        let result: Vec<Value> = a
            .iter()
            .filter(|item| b_set.contains(item))
            .cloned()
            .collect();
        Ok(Value::list(result))
    });

    // list/sliding — sliding window
    register_fn(env, "list/sliding", |args| {
        check_arity!(args, "list/sliding", 2..=3);
        let items = get_sequence(&args[0], "list/sliding")?;
        let size = args[1].as_index("list/sliding")?;
        let step = if args.len() == 3 {
            args[2].as_index("list/sliding")?
        } else {
            1
        };
        if size == 0 {
            return Err(SemaError::eval("list/sliding: size must be positive"));
        }
        if step == 0 {
            return Err(SemaError::eval("list/sliding: step must be positive"));
        }
        let mut result = Vec::new();
        let mut i = 0;
        while i + size <= items.len() {
            result.push(Value::list(items[i..i + size].to_vec()));
            i += step;
        }
        Ok(Value::list(result))
    });

    // list/key-by — turn list of maps into map keyed by fn
    register_hof(
        env,
        "list/key-by",
        |args| {
            check_arity!(args, "list/key-by", 2);
            let items = get_sequence(&args[1], "list/key-by")?;
            let mut map = BTreeMap::new();
            for item in items.iter() {
                let key = call_function(&args[0], &[item.clone()])?;
                map.insert(key, item.clone());
            }
            Ok(Value::map(map))
        },
        |args| {
            check_arity!(args, "list/key-by", 2);
            key_projection_call(
                &args[0],
                &args[1],
                KeyProjectionMode::KeyBy {
                    keyed: BTreeMap::new(),
                },
                "list/key-by",
            )
        },
    );

    // list/times — generate list by calling fn N times
    register_hof(
        env,
        "list/times",
        |args| {
            check_arity!(args, "list/times", 2);
            let n = args[0].as_index("list/times")?;
            let mut result = Vec::with_capacity(n);
            for i in 0..n {
                result.push(call_function(&args[1], &[Value::int(i as i64)])?);
            }
            Ok(Value::list(result))
        },
        |args| {
            check_arity!(args, "list/times", 2);
            let n = args[0].as_index("list/times")?;
            Ok(collect_range_call(
                &args[1],
                n,
                CollectMode::Values,
                "list/times",
            ))
        },
    );

    // list/duplicates — find duplicate values
    register_fn(env, "list/duplicates", |args| {
        check_arity!(args, "list/duplicates", 1);
        let items = get_sequence(&args[0], "list/duplicates")?;
        let mut seen: std::collections::BTreeSet<Value> = std::collections::BTreeSet::new();
        let mut dupes: std::collections::BTreeSet<Value> = std::collections::BTreeSet::new();
        for item in items.iter() {
            if !seen.insert(item.clone()) {
                dupes.insert(item.clone());
            }
        }
        Ok(Value::list(dupes.into_iter().collect()))
    });

    // list/cross-join — cartesian product
    register_fn(env, "list/cross-join", |args| {
        check_arity!(args, "list/cross-join", 2);
        let a = get_sequence(&args[0], "list/cross-join")?;
        let b = get_sequence(&args[1], "list/cross-join")?;
        let mut result = Vec::with_capacity(a.len() * b.len());
        for ai in a.iter() {
            for bi in b.iter() {
                result.push(Value::list(vec![ai.clone(), bi.clone()]));
            }
        }
        Ok(Value::list(result))
    });

    // list/page — pagination
    register_fn(env, "list/page", |args| {
        check_arity!(args, "list/page", 3);
        let items = get_sequence(&args[0], "list/page")?;
        let page = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        let per_page = args[2].as_index("list/page")?;
        if page < 1 {
            return Err(SemaError::eval("list/page: page must be >= 1"));
        }
        let start = ((page - 1) as usize) * per_page;
        if start >= items.len() {
            return Ok(Value::list(vec![]));
        }
        let end = (start + per_page).min(items.len());
        Ok(Value::list(items[start..end].to_vec()))
    });

    // list/find — first matching item
    register_hof(
        env,
        "list/find",
        |args| {
            check_arity!(args, "list/find", 2);
            let items = get_sequence(&args[1], "list/find")?;
            for item in items.iter() {
                let result = call_function(&args[0], &[item.clone()])?;
                if result.is_truthy() {
                    return Ok(item.clone());
                }
            }
            Ok(Value::nil())
        },
        |args| {
            check_arity!(args, "list/find", 2);
            predicate_call(&args[0], &args[1], PredicateMode::Find, "list/find")
        },
    );

    // list/pad — pad list to length
    register_fn(env, "list/pad", |args| {
        check_arity!(args, "list/pad", 3);
        let mut items = get_sequence(&args[0], "list/pad")?.to_vec();
        let target_len = args[1].as_index("list/pad")?;
        let fill = args[2].clone();
        while items.len() < target_len {
            items.push(fill.clone());
        }
        Ok(Value::list(items))
    });

    // list/sole — single matching item or error
    register_hof(
        env,
        "list/sole",
        |args| {
            check_arity!(args, "list/sole", 2);
            let items = get_sequence(&args[1], "list/sole")?;
            let mut found: Option<Value> = None;
            for item in items.iter() {
                let result = call_function(&args[0], &[item.clone()])?;
                if result.is_truthy() {
                    if found.is_some() {
                        return Err(SemaError::eval("list/sole: more than one matching item"));
                    }
                    found = Some(item.clone());
                }
            }
            found.ok_or_else(|| SemaError::eval("list/sole: no matching item"))
        },
        |args| {
            check_arity!(args, "list/sole", 2);
            predicate_call(
                &args[0],
                &args[1],
                PredicateMode::Sole { found: None },
                "list/sole",
            )
        },
    );

    // list/join — join with optional final separator
    register_fn(env, "list/join", |args| {
        check_arity!(args, "list/join", 2..=3);
        let items = get_sequence(&args[0], "list/join")?;
        let sep = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();
        let final_sep = if args.len() == 3 {
            args[2]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?
                .to_string()
        } else {
            sep.clone()
        };
        if items.is_empty() {
            return Ok(Value::string(""));
        }
        use std::fmt::Write;
        let mut out = String::with_capacity(items.len().saturating_mul(8));
        let last = items.len().saturating_sub(1);
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(if i == last { &final_sep } else { &sep });
            }
            write!(&mut out, "{}", v).unwrap();
        }
        Ok(Value::string(&out))
    });

    // tap — side-effect then return original
    register_hof(
        env,
        "tap",
        |args| {
            check_arity!(args, "tap", 2);
            call_function(&args[1], &[args[0].clone()])?;
            Ok(args[0].clone())
        },
        tap_call,
    );

    // Car/cdr compositions (2-deep)
    register_fn(env, "caar", |args| first(&[first(args)?]));
    register_fn(env, "cadr", |args| first(&[rest(args)?]));
    register_fn(env, "cdar", |args| rest(&[first(args)?]));
    register_fn(env, "cddr", |args| rest(&[rest(args)?]));

    // Car/cdr compositions (3-deep)
    register_fn(env, "caaar", |args| first(&[first(&[first(args)?])?]));
    register_fn(env, "caadr", |args| first(&[first(&[rest(args)?])?]));
    register_fn(env, "cadar", |args| first(&[rest(&[first(args)?])?]));
    register_fn(env, "caddr", |args| first(&[rest(&[rest(args)?])?]));
    register_fn(env, "cdaar", |args| rest(&[first(&[first(args)?])?]));
    register_fn(env, "cdadr", |args| rest(&[first(&[rest(args)?])?]));
    register_fn(env, "cddar", |args| rest(&[rest(&[first(args)?])?]));
    register_fn(env, "cdddr", |args| rest(&[rest(&[rest(args)?])?]));

    // Association list functions (assoc is dual-purpose in map.rs)
    register_fn(env, "assq", |args| {
        check_arity!(args, "assq", 2);
        let key = &args[0];
        let alist = get_sequence(&args[1], "assq")?;
        for pair in alist.iter() {
            if let Some(p) = pair.as_list() {
                if !p.is_empty() && &p[0] == key {
                    return Ok(pair.clone());
                }
            }
        }
        Ok(Value::bool(false))
    });

    register_fn(env, "assv", |args| {
        check_arity!(args, "assv", 2);
        let key = &args[0];
        let alist = get_sequence(&args[1], "assv")?;
        for pair in alist.iter() {
            if let Some(p) = pair.as_list() {
                if !p.is_empty() && &p[0] == key {
                    return Ok(pair.clone());
                }
            }
        }
        Ok(Value::bool(false))
    });

    // Silent aliases for other Lisp dialects (undocumented)
    if let Some(v) = env.get(sema_core::intern("map")) {
        env.set(sema_core::intern("mapcar"), v);
    }
    if let Some(v) = env.get(sema_core::intern("foldl")) {
        env.set(sema_core::intern("fold"), v);
    }
    if let Some(v) = env.get(sema_core::intern("any")) {
        env.set(sema_core::intern("some?"), v.clone());
        env.set(sema_core::intern("any?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("every")) {
        env.set(sema_core::intern("every?"), v);
    }
}

fn first(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "car", 1);
    if let Some(l) = args[0].as_list() {
        if l.is_empty() {
            Ok(Value::nil())
        } else {
            Ok(l[0].clone())
        }
    } else if let Some(v) = args[0].as_vector() {
        if v.is_empty() {
            Ok(Value::nil())
        } else {
            Ok(v[0].clone())
        }
    } else {
        Err(SemaError::type_error("list or vector", args[0].type_name())
            .with_hint("first: argument 1 must be a list or vector"))
    }
}

fn rest(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "cdr", 1);
    if let Some(l) = args[0].as_list() {
        if l.len() <= 1 {
            Ok(Value::list(vec![]))
        } else {
            Ok(Value::list(l[1..].to_vec()))
        }
    } else if let Some(v) = args[0].as_vector() {
        if v.len() <= 1 {
            Ok(Value::vector(vec![]))
        } else {
            Ok(Value::vector(v[1..].to_vec()))
        }
    } else {
        Err(SemaError::type_error("list or vector", args[0].type_name())
            .with_hint("rest: argument 1 must be a list or vector"))
    }
}

/// Coerce a sequence argument into a slice for iteration.
///
/// Lists and vectors are immutable `Rc<Vec<Value>>`, so they are borrowed
/// zero-copy (`Cow::Borrowed`). A `mutable-array` wraps a `RefCell<Vec<Value>>`
/// whose `Ref` guard would outlive this call, so it is snapshotted into an
/// owned `Vec` (`Cow::Owned`). Snapshotting up front is also what makes the
/// callback-driven HOFs (map/filter/for-each) reentrancy-safe: a callback that
/// mutates the same array cannot hit an "already borrowed" panic, and iteration
/// ranges over the array as it stood when the HOF was entered.
fn get_sequence<'a>(val: &'a Value, ctx: &str) -> Result<Cow<'a, [Value]>, SemaError> {
    if let Some(l) = val.as_list() {
        Ok(Cow::Borrowed(l))
    } else if let Some(v) = val.as_vector() {
        Ok(Cow::Borrowed(v))
    } else if let Some(arr) = val.as_mutable_array() {
        Ok(Cow::Owned(arr.items.borrow().clone()))
    } else {
        Err(SemaError::type_error(
            "list, vector, or mutable-array",
            format!("{} in {ctx}", val.type_name()),
        )
        .with_hint(format!(
            "{ctx}: expected a list, vector, or mutable-array to iterate over"
        )))
    }
}

fn flatten_recursive(val: &Value, out: &mut Vec<Value>) {
    if let Some(l) = val.as_list() {
        for item in l.iter() {
            flatten_recursive(item, out);
        }
    } else if let Some(v) = val.as_vector() {
        for item in v.iter() {
            flatten_recursive(item, out);
        }
    } else {
        out.push(val.clone());
    }
}

fn num_lt(a: &Value, b: &Value) -> Result<bool, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(a), ValueViewRef::Int(b)) => Ok(a < b),
        (ValueViewRef::Float(a), ValueViewRef::Float(b)) => Ok(a < b),
        (ValueViewRef::Int(a), ValueViewRef::Float(b)) => Ok((a as f64) < b),
        (ValueViewRef::Float(a), ValueViewRef::Int(b)) => Ok(a < (b as f64)),
        _ => Err(SemaError::type_error("number", a.type_name())),
    }
}

/// True when `v` is a genuinely runtime-only native — its legacy value ABI is
/// the "requires runtime invocation" hard-error stub, so the cooperative
/// `NativeOutcome::Call` path is its only viable path. Synchronous compatibility
/// entry points use this to produce an actionable error before reaching that
/// stub; runtime callback drivers structurally invoke every callable.
fn is_runtime_only_native(v: &Value) -> bool {
    v.as_native_fn_rc()
        .is_some_and(|native| native.is_runtime_only())
}

/// A runtime-only native (`async/spawn`, `channel/*`, `async/resolved`, …) can
/// only run on the cooperative `NativeOutcome::Call` path. `apply` /
/// `call-with-values` reach that path on their runtime ABI, but when they are
/// invoked SYNCHRONOUSLY (the value ABI — e.g. as the callee of another `apply`,
/// or at a bare top level with no runtime quantum) there is no way to suspend,
/// so calling the native would hit its "requires runtime invocation" value-ABI
/// stub. Raise a clear, actionable error instead of leaking that internal stub.
fn runtime_only_sync_apply_err(func: &Value, via: &str) -> SemaError {
    let name = func
        .as_native_fn_rc()
        .map(|n| n.name.clone())
        .unwrap_or_else(|| "<native>".to_string());
    SemaError::eval(format!(
        "cannot invoke runtime-only native '{name}' through a synchronous \
         `{via}` — call it directly (e.g. `({name} …)`) or wrap it in a lambda \
         so the runtime can drive it",
    ))
}

/// True when `v` can be applied as a function — a native fn (including a
/// VM-closure wrapper), a keyword (keyword-as-getter), or a multimethod. Mirrors
/// the callable arms of the evaluator's `call_value`. `call-with-values` uses it
/// to keep a non-callable producer on the exact legacy `call_function` error
/// path rather than surfacing it through the runtime's callable check.
fn is_callable(v: &Value) -> bool {
    v.is_native_fn() || v.is_keyword() || v.as_multimethod_rc().is_some()
}

/// Call a Sema function (lambda or native) with given args.
/// Delegates to the real evaluator via the registered callback.
///
/// This is the synchronous callback seam used by dual-ABI builtins' plain value
/// implementations and host-only entry points. Runtime callback drivers issue a
/// structural `NativeOutcome::Call` instead, which lets runtime-only and dual-ABI
/// natives suspend on the active task.
pub fn call_function(func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    if let Some(native) = func.as_native_fn_rc() {
        sema_core::with_stdlib_ctx(|ctx| {
            if native.escaping_args().is_empty() {
                (native.func)(ctx, args)
            } else {
                sema_core::call_callback(ctx, func, args)
            }
        })
    } else {
        sema_core::with_stdlib_ctx(|ctx| sema_core::call_callback(ctx, func, args))
    }
}

/// [`call_function`] with an args buffer the caller owns and will not reuse:
/// a VM-closure callee moves the values into its frame (the buffer is left
/// holding nils), keeping a fold accumulator uniquely owned across the
/// callback boundary so the `strong_count == 1` in-place fast paths can fire.
pub fn call_function_owned(func: &Value, args: &mut [Value]) -> Result<Value, SemaError> {
    sema_core::with_stdlib_ctx(|ctx| sema_core::call_callback_owned(ctx, func, args))
}

#[cfg(test)]
mod continuation_trace_tests {
    use super::*;
    use std::collections::VecDeque;

    fn edge_count(trace: &dyn Trace) -> usize {
        let mut count = 0;
        assert!(trace.trace(&mut |_| count += 1));
        count
    }

    /// Cooperative callback continuations carry `Value` state across callback
    /// boundaries, so their GC trace must emit exactly one edge per retained
    /// value.
    #[test]
    fn collect_continuation_emits_one_edge_per_value() {
        let cont = CollectContinuation {
            hof: "map",
            callback: Value::string("f"),
            remaining: CollectCalls::Variadic(VecDeque::from(vec![
                vec![Value::int(1), Value::int(2)],
                vec![Value::int(3), Value::int(4)],
            ])),
            results: CollectResults::Values(vec![Value::int(5)]),
        };
        // callback (1) + 4 remaining column items + 1 result = 6.
        assert_eq!(edge_count(&cont), 6);
    }

    #[test]
    fn compact_collect_sources_trace_only_their_retained_values() {
        let indexed = CollectContinuation {
            hof: "map-indexed",
            callback: Value::string("f"),
            remaining: CollectCalls::Indexed {
                items: vec![Value::int(1), Value::int(2)],
                next: 0,
            },
            results: CollectResults::Values(Vec::new()),
        };
        assert_eq!(edge_count(&indexed), 3);

        let string = CollectContinuation {
            hof: "string/map",
            callback: Value::string("f"),
            remaining: CollectCalls::String {
                source: Value::string("abc"),
                byte_index: 0,
            },
            results: CollectResults::String(String::new()),
        };
        assert_eq!(edge_count(&string), 2);

        let range = CollectContinuation {
            hof: "list/times",
            callback: Value::string("f"),
            remaining: CollectCalls::Range { next: 0, end: 3 },
            results: CollectResults::Values(Vec::new()),
        };
        assert_eq!(edge_count(&range), 1);
    }

    #[test]
    fn predicate_continuation_traces_inputs_and_accumulated_values() {
        let partition = PredicateContinuation {
            hof: "partition",
            predicate: Value::string("predicate"),
            current: Value::int(1),
            remaining: PredicateItems::Snapshot {
                items: vec![Value::int(2), Value::int(3)],
                next: 0,
            },
            mode: PredicateMode::Partition {
                matching: vec![Value::int(4)],
                non_matching: vec![Value::int(5)],
            },
        };
        // Predicate + current + two remaining + two accumulated values.
        assert_eq!(edge_count(&partition), 6);

        let sole = PredicateContinuation {
            hof: "list/sole",
            predicate: Value::string("predicate"),
            current: Value::int(1),
            remaining: PredicateItems::Retained {
                source: Value::list(vec![Value::int(1), Value::int(2)]),
                next: 1,
            },
            mode: PredicateMode::Sole {
                found: Some(Value::int(2)),
            },
        };
        // Predicate + current + one compact source handle + retained sole match.
        assert_eq!(edge_count(&sole), 4);
    }

    #[test]
    fn key_projection_continuation_traces_inputs_and_outputs() {
        let grouped = KeyProjectionContinuation {
            hof: "list/group-by",
            key_fn: Value::string("key-fn"),
            current: Value::int(1),
            remaining: PredicateItems::Retained {
                source: Value::list(vec![Value::int(1), Value::int(2)]),
                next: 1,
            },
            mode: KeyProjectionMode::GroupBy {
                groups: vec![(Value::keyword("group"), vec![Value::int(3), Value::int(4)])],
            },
        };
        // Key fn + current + compact source + group key + two grouped items.
        assert_eq!(edge_count(&grouped), 6);

        let keyed = KeyProjectionContinuation {
            hof: "list/key-by",
            key_fn: Value::string("key-fn"),
            current: Value::int(1),
            remaining: PredicateItems::Snapshot {
                items: vec![Value::nil(), Value::int(2)],
                next: 1,
            },
            mode: KeyProjectionMode::KeyBy {
                keyed: BTreeMap::from([(Value::keyword("key"), Value::int(3))]),
            },
        };
        // Key fn + current + one pending snapshot item + output key/item.
        assert_eq!(edge_count(&keyed), 5);
    }

    #[test]
    fn fold_continuation_traces_pending_inputs() {
        let values = FoldContinuation {
            hof: "foldr",
            combiner: Value::string("combiner"),
            remaining: FoldItems::Sequence {
                items: FoldSequenceItems::Retained {
                    source: Value::list(vec![Value::int(1), Value::int(2)]),
                    start: 0,
                    end: 2,
                },
                direction: FoldDirection::Reverse,
            },
            order: FoldOrder::ItemThenAccumulator,
        };
        assert_eq!(edge_count(&values), 2);

        let snapshot = FoldContinuation {
            hof: "foldl",
            combiner: Value::string("combiner"),
            remaining: FoldItems::Sequence {
                items: FoldSequenceItems::Snapshot {
                    items: vec![Value::int(1), Value::int(2)],
                    start: 0,
                    end: 2,
                },
                direction: FoldDirection::Forward,
            },
            order: FoldOrder::AccumulatorThenItem,
        };
        assert_eq!(edge_count(&snapshot), 3);

        let typed = FoldContinuation {
            hof: "i64-array/fold",
            combiner: Value::string("combiner"),
            remaining: FoldItems::I64Array {
                source: Value::i64_array(vec![1, 2]),
                next: 1,
            },
            order: FoldOrder::AccumulatorThenItem,
        };
        assert_eq!(edge_count(&typed), 2);
    }

    #[test]
    fn identity_continuation_holds_no_value_edges() {
        // Only a `&'static str` tag — no `Value` state.
        assert_eq!(edge_count(&IdentityContinuation { hof: "apply" }), 0);
    }

    #[test]
    fn tap_continuation_traces_the_original_value() {
        let cont = TapContinuation {
            original: Value::string("original"),
        };
        assert_eq!(edge_count(&cont), 1);
    }

    #[test]
    fn call_with_values_continuation_emits_one_edge_for_consumer() {
        let cont = CallWithValuesContinuation {
            consumer: Value::string("consumer"),
        };
        assert_eq!(edge_count(&cont), 1);
    }

    #[test]
    fn sort_comparator_continuation_traces_every_unfinished_value() {
        let cont = SortComparatorContinuation {
            comparator: Value::string("comparator"),
            runs: VecDeque::from([VecDeque::from([Value::int(1), Value::int(2)])]),
            next_runs: vec![vec![Value::int(3)]],
            left: VecDeque::from([Value::int(4)]),
            right: VecDeque::from([Value::int(5)]),
            merged: vec![Value::int(6)],
        };
        // Comparator + two pending-run items + one completed-run item + both
        // active fronts + one merged item.
        assert_eq!(edge_count(&cont), 7);
    }
}
