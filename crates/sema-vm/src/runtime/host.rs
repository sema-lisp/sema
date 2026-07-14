//! Minimal production host adapters for driving the runtime from an interpreter.
//!
//! `MonotonicClock` is the real wall-clock source; `NullExecutor` accepts no
//! external work and is suitable for evaluating purely synchronous roots (which
//! never submit I/O). A real I/O executor replaces `NullExecutor` when the
//! async/resource layers are wired through the runtime.

use std::sync::Arc;
use std::time::Instant;

use sema_core::runtime::{
    ExecutorAttachError, ExecutorLease, ExecutorShutdown, ExecutorSnapshot, ExecutorSubmission,
    IoExecutor, RunningSubmission, RuntimeId, SubmissionRejected, SubmitErrorKind,
};

use super::RuntimeClock;

/// Production monotonic clock backed by `Instant::now()`.
pub struct MonotonicClock;

impl RuntimeClock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Executor that accepts no external work: any submission is rejected. A runtime
/// built with it can drive synchronous roots (which never submit I/O) but cannot
/// service real external operations.
pub struct NullExecutor;

struct NullLease;

impl IoExecutor for NullExecutor {
    fn attach_runtime(
        &self,
        _runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        Ok(Arc::new(NullLease))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }
}

impl ExecutorLease for NullLease {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        Err(submission.reject(SubmitErrorKind::Capacity))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        ExecutorSnapshot::default()
    }

    fn shutdown(&self, _deadline: Instant) -> ExecutorShutdown {
        ExecutorShutdown::Drained(ExecutorSnapshot::default())
    }
}
