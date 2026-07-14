use std::num::NonZeroU64;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("quarantine hard deadline must be nonzero")]
pub struct InvalidQuarantineBound;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineBoundDescriptor {
    HardDeadline(Duration),
    FiniteWork {
        kind: &'static str,
        maximum_units: NonZeroU64,
    },
}

pub struct QuarantineBound(QuarantineBoundDescriptor);

impl QuarantineBound {
    pub fn hard_deadline(deadline: Duration) -> Result<Self, InvalidQuarantineBound> {
        if deadline.is_zero() {
            return Err(InvalidQuarantineBound);
        }
        Ok(Self(QuarantineBoundDescriptor::HardDeadline(deadline)))
    }

    pub fn finite_work(kind: &'static str, maximum_units: NonZeroU64) -> Self {
        Self(QuarantineBoundDescriptor::FiniteWork {
            kind,
            maximum_units,
        })
    }

    pub fn descriptor(&self) -> QuarantineBoundDescriptor {
        self.0
    }

    pub fn hard_deadline_value(&self) -> Option<Duration> {
        match self.0 {
            QuarantineBoundDescriptor::HardDeadline(value) => Some(value),
            QuarantineBoundDescriptor::FiniteWork { .. } => None,
        }
    }

    pub fn finite_work_value(&self) -> Option<(&'static str, NonZeroU64)> {
        match self.0 {
            QuarantineBoundDescriptor::HardDeadline(_) => None,
            QuarantineBoundDescriptor::FiniteWork {
                kind,
                maximum_units,
            } => Some((kind, maximum_units)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelDisposition {
    Reaped,
    PendingReap,
}

#[derive(Debug, thiserror::Error)]
#[error("resource cancellation hook failed: {message}")]
pub struct CancelHookError {
    message: String,
}

impl CancelHookError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub trait CancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError>;
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError>;
}

pub struct InterruptibleResource {
    kind: &'static str,
    hook: Box<dyn CancelHook>,
}

impl InterruptibleResource {
    pub fn new(kind: &'static str, hook: Box<dyn CancelHook>) -> Self {
        Self { kind, hook }
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub(crate) fn into_parts(self) -> (&'static str, Box<dyn CancelHook>) {
        (self.kind, self.hook)
    }
}

pub enum ResourceClass {
    Interruptible {
        kind: &'static str,
        hook: Box<dyn CancelHook>,
    },
    QuarantinedBounded(QuarantineBound),
}
