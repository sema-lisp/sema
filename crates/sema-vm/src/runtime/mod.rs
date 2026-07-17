mod channel;
mod drive;
mod host;
mod host_api;
mod promise;
mod ready;
mod resource_gate;
mod root;
mod state;
mod task;
mod timer;
mod wait;

pub use drive::{BoundedDriver, DriveBudget, DriveReport, DriveState, RuntimeClock};
pub use host::{MonotonicClock, NullExecutor, ThreadPoolExecutor};
pub use host_api::{
    OutputEvent, RootHandle, RootOptions, RootPoll, RuntimeCommandHandle, ShutdownInvariantFailure,
    ShutdownOptions, ShutdownReport,
};
pub use ready::ReadyScheduler;
pub use root::{RootRecord, RootState, RootTransitionError};
#[cfg(test)]
use state::TestPreparedTask;
pub use state::{Runtime, RuntimeFault, SubmitRootError};
pub use task::{
    CancellationRequest, ContinuationFrame, StateName, TaskRecord, TaskState, TaskTransitionError,
    WaitKey,
};
pub use timer::TimerQueue;
pub use wait::{
    CleanupDiagnostic, CompletionRoute, PendingResume, RegisterExternalError, RuntimeCreateError,
    WaitRuntime,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use channel::CancelledChannelWait;
pub(crate) use channel::{ChannelRegistry, ChannelResult};
pub(crate) use promise::{PromiseRegistry, PromiseState, RegistryError};
pub(crate) use resource_gate::{AcquireResult, GateResult, ResourceGateRegistry, ResourceGateWake};
