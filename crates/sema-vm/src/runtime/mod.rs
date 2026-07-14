mod root;
mod task;

pub use root::{RootRecord, RootState, RootTransitionError};
pub use task::{
    CancellationRequest, StateName, TaskRecord, TaskState, TaskTransitionError, WaitKey,
};

#[cfg(test)]
mod tests;
