//! Minimal `sema_core::runtime` implementations for executor tests: a
//! channel-backed completion sink, a decoder that yields nil, a cancel hook
//! that always reports "reaped", and a blocking sleep operation built from
//! them. Compiled for this crate's own tests and for dependants that enable
//! the `test-support` feature.

use std::sync::Mutex;
use std::time::Duration;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    CancelDisposition, CancelHook, CancelHookError, CompletionDecoder, CompletionDelivery,
    CompletionKind, CompletionSender, DecodedCompletion, ExternalCompletion, ExternalFailure,
    InterruptibleResource, NativeCallContext, PreparedExternalOperation, SendPayload, Trace,
};
use sema_core::Value;

/// Delivers completions into an `mpsc` channel the test reads from.
pub struct ChannelSender(pub Mutex<std::sync::mpsc::Sender<ExternalCompletion>>);

impl CompletionSender for ChannelSender {
    fn send(&self, completion: ExternalCompletion) -> CompletionDelivery {
        self.0
            .lock()
            .unwrap()
            .send(completion)
            .map(|()| CompletionDelivery::Delivered)
            .unwrap_or(CompletionDelivery::InboxClosed)
    }
}

/// Decodes every completion to nil.
pub struct NilDecoder;
impl Trace for NilDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}
impl CompletionDecoder for NilDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        _result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        Ok(Value::nil())
    }
}

/// A cancel hook that reports the resource as already reaped.
pub struct NoopHook;
impl Trace for NoopHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}
impl CancelHook for NoopHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// An interruptible blocking operation that sleeps `ms` milliseconds.
pub fn blocking_sleep_op(ms: u64) -> PreparedExternalOperation {
    PreparedExternalOperation::interruptible_blocking(
        CompletionKind::try_from_raw(1).unwrap(),
        Box::new(NilDecoder),
        InterruptibleResource::new("sleep", Box::new(NoopHook)),
        move || {
            std::thread::sleep(Duration::from_millis(ms));
            Ok(Box::new(()) as SendPayload)
        },
    )
}
