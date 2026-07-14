use crate::{SemaError, Value};

use super::{CancelReason, SettlementSeq};

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
