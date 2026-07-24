use std::any::{type_name, Any};

use crate::{SemaError, Value};

use super::{
    CompletionKind, NativeCallContext, OperationId, RuntimeId, Trace, WaitGeneration, WaitId,
};

pub type SendPayload = Box<dyn Any + Send>;
pub type DecodedCompletion = Result<Value, SemaError>;

pub trait CompletionDecoder: Trace {
    fn decode(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion;
}

pub struct ExternalCompletion {
    pub runtime_id: RuntimeId,
    pub wait_id: WaitId,
    pub generation: WaitGeneration,
    pub operation_id: OperationId,
    pub kind: CompletionKind,
    pub result: Result<SendPayload, ExternalFailure>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalFailureCode {
    Rejected,
    Cancelled,
    DeadlineExceeded,
    BoundExceeded,
    WorkerPanic,
    Decode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalFailure {
    code: ExternalFailureCode,
    message: String,
    operation: Option<&'static str>,
    expected_type: Option<&'static str>,
}

impl ExternalFailure {
    fn new(code: ExternalFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            operation: None,
            expected_type: None,
        }
    }

    pub fn code(&self) -> ExternalFailureCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn operation(&self) -> Option<&'static str> {
        self.operation
    }

    pub fn expected_type(&self) -> Option<&'static str> {
        self.expected_type
    }

    pub fn deadline_exceeded(message: impl Into<String>) -> Self {
        Self::new(ExternalFailureCode::DeadlineExceeded, message)
    }

    pub fn bound_exceeded(message: impl Into<String>) -> Self {
        Self::new(ExternalFailureCode::BoundExceeded, message)
    }

    /// The runtime-side failure used after an executor rejects an unadmitted submission.
    pub fn rejected() -> Self {
        Self::new(ExternalFailureCode::Rejected, "external operation rejected")
    }

    pub(crate) fn decode(
        message: String,
        operation: &'static str,
        expected_type: &'static str,
    ) -> Self {
        Self {
            code: ExternalFailureCode::Decode,
            message,
            operation: Some(operation),
            expected_type: Some(expected_type),
        }
    }

    pub(crate) fn cancelled() -> Self {
        Self::new(
            ExternalFailureCode::Cancelled,
            "external operation cancelled",
        )
    }

    pub(crate) fn worker_panic() -> Self {
        Self::new(ExternalFailureCode::WorkerPanic, "external worker panicked")
    }
}

pub fn downcast_send_payload<T: Any + Send>(
    payload: SendPayload,
    operation: &'static str,
) -> Result<T, ExternalFailure> {
    payload.downcast::<T>().map(|value| *value).map_err(|_| {
        let expected = type_name::<T>();
        ExternalFailure::decode(
            format!("{operation} returned an unexpected payload; expected {expected}"),
            operation,
            expected,
        )
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionDelivery {
    Delivered,
    InboxClosed,
}

pub trait CompletionSender: Send + Sync + 'static {
    fn send(&self, completion: ExternalCompletion) -> CompletionDelivery;
}
