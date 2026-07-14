use super::{CancelReason, TaskContext};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CancellationView {
    requested: bool,
    reason: Option<CancelReason>,
}

impl CancellationView {
    #[doc(hidden)]
    pub fn new(requested: bool, reason: Option<CancelReason>) -> Self {
        Self { requested, reason }
    }

    pub fn is_requested(&self) -> bool {
        self.requested
    }

    pub fn reason(&self) -> Option<&CancelReason> {
        self.reason.as_ref()
    }
}

pub struct NativeCallContext<'a> {
    pub task_context: &'a mut TaskContext,
    pub cancellation: CancellationView,
}
