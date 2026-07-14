use crate::{SemaError, Value};

use super::{CancelReason, SettlementSeq, Trace};
use crate::cycle::GcEdge;

#[derive(Debug)]
pub enum TaskOutcome {
    Returned(Value),
    Failed(SemaError),
    Cancelled(CancelReason),
}

#[derive(Debug)]
pub struct TaskSettlement {
    pub sequence: SettlementSeq,
    pub outcome: TaskOutcome,
}

impl Trace for TaskOutcome {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Returned(value) => sink(GcEdge::Value(value)),
            Self::Failed(error) => trace_error(error, sink),
            Self::Cancelled(_) => {}
        }
        true
    }
}

fn trace_error(error: &SemaError, sink: &mut dyn FnMut(GcEdge<'_>)) {
    match error {
        SemaError::UserException(value) | SemaError::Condition(value) => {
            sink(GcEdge::Value(value));
        }
        SemaError::WithTrace { inner, .. } | SemaError::WithContext { inner, .. } => {
            trace_error(inner, sink);
        }
        _ => {}
    }
}

impl Trace for SemaError {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        trace_error(self, sink);
        true
    }
}

impl Trace for TaskSettlement {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.outcome.trace(sink)
    }
}
